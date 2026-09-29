//! Daemon-derived run progress (ADR-0004).
//!
//! The daemon composes the progress text from two facts it holds: the
//! run state, and the tool call in flight. There is no `set_progress`
//! tool, so a model can neither pay for it in prompt tax nor lie about
//! its own progress. There is no step counter, because the daemon
//! holds no fact that supplies "step 3 of 7".
//!
//! Updates ride ephemeral WS frames, gated on channel subscription
//! like `message.delta`, and never become `events` rows. Only the
//! terminal text persists, in the `progress` block of the run's
//! progress message, so a finished run shows its last state and not a
//! replay of the ticks.

use std::collections::HashMap;
use std::sync::Mutex;

use pagis_core::{AgentId, ChannelId, MessageId, RunId, RunState};
use serde::Serialize;
use tokio::sync::broadcast;
use utoipa::ToSchema;

const BROADCAST_CAPACITY: usize = 1024;

/// A tool label longer than this is cut, so one progress line stays
/// one line.
const LABEL_CAP: usize = 72;

/// One `progress.state` WS payload: the whole current text, never a
/// delta. A client applies a frame only when `seq` is newer than the
/// one it holds, so a catch-up never loses to a stale live frame.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ProgressFrame {
    pub channel_id: String,
    pub message_id: String,
    pub run_id: String,
    /// The agent working; the client names a synthetic row with it.
    pub agent_id: String,
    /// Set when the run is bound to a thread.
    pub parent_message_id: Option<String>,
    pub seq: u64,
    pub text: String,
}

/// The live progress of one run.
struct LiveProgress {
    channel_id: ChannelId,
    message_id: MessageId,
    agent_id: AgentId,
    parent_message_id: Option<MessageId>,
    state: RunState,
    /// The tool in flight, as the line names it.
    tool: Option<String>,
    /// True while the Agent replaces older conversation context before
    /// it can continue the reply.
    preparing_context: bool,
    seq: u64,
}

impl LiveProgress {
    fn frame(&self, run_id: &RunId) -> ProgressFrame {
        ProgressFrame {
            channel_id: self.channel_id.to_string(),
            message_id: self.message_id.to_string(),
            run_id: run_id.to_string(),
            agent_id: self.agent_id.to_string(),
            parent_message_id: self.parent_message_id.as_ref().map(|p| p.to_string()),
            seq: self.seq,
            text: if self.preparing_context {
                "Preparing conversation…".to_string()
            } else {
                compose(self.state, self.tool.as_deref())
            },
        }
    }
}

/// What the settled progress of one run writes: the message that
/// carries the block, and the terminal text.
pub struct ProgressEnd {
    pub message_id: MessageId,
    pub channel_id: ChannelId,
    pub parent_message_id: Option<MessageId>,
    pub text: String,
}

/// The progress hub: the agent loop writes, WebSocket sessions read.
pub struct ProgressHub {
    live: Mutex<HashMap<RunId, LiveProgress>>,
    tx: broadcast::Sender<ProgressFrame>,
}

impl Default for ProgressHub {
    fn default() -> Self {
        let (tx, _) = broadcast::channel(BROADCAST_CAPACITY);
        Self {
            live: Mutex::new(HashMap::new()),
            tx,
        }
    }
}

impl ProgressHub {
    /// Open the progress of one run, against the message the terminal
    /// text lands in, and broadcast its opening line.
    pub fn begin(
        &self,
        run_id: RunId,
        channel_id: ChannelId,
        message_id: MessageId,
        agent_id: AgentId,
        parent_message_id: Option<MessageId>,
        state: RunState,
    ) {
        let mut live = self.live.lock().expect("progress lock");
        let progress = LiveProgress {
            channel_id,
            message_id,
            agent_id,
            parent_message_id,
            state,
            tool: None,
            preparing_context: false,
            seq: 1,
        };
        let _ = self.tx.send(progress.frame(&run_id));
        live.insert(run_id, progress);
    }

    /// The run changed state: recompose and broadcast.
    pub fn state_changed(&self, run_id: &RunId, state: RunState) {
        self.update(run_id, |progress| progress.state = state);
    }

    /// A tool call started (`Some`) or finished (`None`).
    pub fn tool_changed(&self, run_id: &RunId, tool: Option<String>) {
        self.update(run_id, |progress| progress.tool = tool);
    }

    /// Show or clear the context work that must finish before the reply
    /// can continue. The Run stays `running`, so Stop and the composer
    /// keep their current behavior.
    pub fn context_compaction_changed(&self, run_id: &RunId, active: bool) {
        self.update(run_id, |progress| progress.preparing_context = active);
    }

    /// Close the run's progress: broadcast the terminal line, then
    /// drop the run. The last frame carries what the block now holds,
    /// so a client that already folded it sees no change when the
    /// settled row arrives. Returns the message that carries the block
    /// and the terminal text to persist in it.
    pub fn end(&self, run_id: &RunId, state: RunState) -> Option<ProgressEnd> {
        let mut live = self.live.lock().expect("progress lock");
        let mut progress = live.remove(run_id)?;
        progress.state = state;
        progress.tool = None;
        progress.seq += 1;
        let frame = progress.frame(run_id);
        let _ = self.tx.send(frame.clone());
        Some(ProgressEnd {
            message_id: progress.message_id,
            channel_id: progress.channel_id,
            parent_message_id: progress.parent_message_id,
            text: frame.text,
        })
    }

    /// The current progress of every live run in the channel, as one
    /// frame each: the mid-run catch-up a new subscriber reads.
    pub fn snapshot(&self, channel_id: &str) -> Vec<ProgressFrame> {
        let live = self.live.lock().expect("progress lock");
        live.iter()
            .filter(|(_, progress)| progress.channel_id.as_str() == channel_id)
            .map(|(run_id, progress)| progress.frame(run_id))
            .collect()
    }

    pub fn subscribe(&self) -> broadcast::Receiver<ProgressFrame> {
        self.tx.subscribe()
    }

    /// Apply one change and broadcast the new state. The broadcast
    /// happens under the lock, so a snapshot never races a frame out
    /// of order.
    fn update(&self, run_id: &RunId, change: impl FnOnce(&mut LiveProgress)) {
        let mut live = self.live.lock().expect("progress lock");
        let Some(progress) = live.get_mut(run_id) else {
            return;
        };
        change(progress);
        progress.seq += 1;
        let _ = self.tx.send(progress.frame(run_id));
    }
}

/// The progress line for one run state and the tool in flight.
pub fn compose(state: RunState, tool: Option<&str>) -> String {
    match (state, tool) {
        (RunState::Running, Some(tool)) => format!("Running `{tool}`…"),
        (RunState::Running, None) => "Thinking…".to_string(),
        (RunState::Reflecting, _) => "Updating memory".to_string(),
        (RunState::Queued, _) => "Queued".to_string(),
        (RunState::WaitingForApproval, _) => "Waiting for your approval".to_string(),
        (RunState::WaitingForUser, _) => "Waiting for you".to_string(),
        (RunState::Completed, _) => "Done".to_string(),
        (RunState::Failed, _) => "Failed".to_string(),
        (RunState::Canceled, _) => "Stopped".to_string(),
    }
}

/// How a progress line names one tool call. A host command and a
/// Software Package call are acts on named things, so the line names
/// the thing: the command, the package, the fork, the Contribution.
/// Every other tool shows its name, because its arguments are not the
/// daemon's to read out.
pub fn tool_label(name: &str, arguments: &str) -> String {
    let arguments = serde_json::from_str::<serde_json::Value>(arguments).unwrap_or_default();
    let text = |field: &str| {
        arguments[field]
            .as_str()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    };
    let label = match name {
        pagis_broker::HOST_SHELL => text("command"),
        pagis_broker::TOOL_SEARCH => text("query").map(|query| format!("{name} '{query}'")),
        pagis_broker::SOFTWARE_PUBLISH => text("name").map(|package| format!("{name} {package}")),
        pagis_broker::SOFTWARE_FORK => match (text("source"), text("new_name")) {
            (Some(source), Some(fork)) => Some(format!("{name} {source} as {fork}")),
            _ => None,
        },
        pagis_broker::SOFTWARE_CONTRIBUTE => {
            text("package").map(|package| format!("{name} {package}"))
        }
        pagis_broker::CONTRIBUTION_VIEW | pagis_broker::CONTRIBUTION_CLOSE => {
            text("id").map(|id| format!("{name} {id}"))
        }
        _ => None,
    };
    match label {
        Some(label) => cut(&label),
        None => name.to_string(),
    }
}

/// Cut a label to one line's worth of characters.
fn cut(label: &str) -> String {
    if label.chars().count() <= LABEL_CAP {
        return label.to_string();
    }
    let head: String = label.chars().take(LABEL_CAP).collect();
    format!("{head}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hub_with_run() -> (ProgressHub, RunId) {
        let hub = ProgressHub::default();
        let run_id = RunId::from("run_1".to_string());
        hub.begin(
            run_id.clone(),
            ChannelId::from("chn_1".to_string()),
            MessageId::from("msg_1".to_string()),
            AgentId::from("agt_1".to_string()),
            None,
            RunState::Running,
        );
        (hub, run_id)
    }

    #[test]
    fn a_running_run_with_no_tool_is_thinking() {
        assert_eq!(compose(RunState::Running, None), "Thinking…");
    }

    #[test]
    fn a_host_command_names_itself() {
        let label = tool_label(
            "host_shell",
            &serde_json::json!({"command": "git status"}).to_string(),
        );
        assert_eq!(
            compose(RunState::Running, Some(&label)),
            "Running `git status`…"
        );
    }

    #[test]
    fn another_tool_names_the_tool() {
        let label = tool_label(
            "memory_write",
            &serde_json::json!({"path": "shared/x.md"}).to_string(),
        );
        assert_eq!(label, "memory_write");
    }

    #[test]
    fn a_software_call_names_what_it_acts_on() {
        assert_eq!(
            tool_label(
                "tool_search",
                &serde_json::json!({"query": "weather"}).to_string()
            ),
            "tool_search 'weather'"
        );
        assert_eq!(
            tool_label(
                "software_publish",
                &serde_json::json!({"name": "weather", "notes": "hourly"}).to_string()
            ),
            "software_publish weather"
        );
        assert_eq!(
            tool_label(
                "software_fork",
                &serde_json::json!({"source": "weather@v3", "new_name": "weather-ada"}).to_string()
            ),
            "software_fork weather@v3 as weather-ada"
        );
        assert_eq!(
            tool_label(
                "software_contribute",
                &serde_json::json!({"package": "weather", "summary": "fix"}).to_string()
            ),
            "software_contribute weather"
        );
        assert_eq!(
            tool_label(
                "contribution_close",
                &serde_json::json!({"id": "c_12", "status": "merged"}).to_string()
            ),
            "contribution_close c_12"
        );
        assert_eq!(
            compose(RunState::Running, Some("software_publish weather")),
            "Running `software_publish weather`…"
        );
    }

    #[test]
    fn a_software_tool_of_a_package_names_itself() {
        assert_eq!(
            tool_label(
                "weather__forecast",
                &serde_json::json!({"city": "Paris"}).to_string()
            ),
            "weather__forecast"
        );
    }

    #[test]
    fn a_browse_of_the_software_list_names_the_tool() {
        assert_eq!(
            tool_label("tool_search", &serde_json::json!({"query": ""}).to_string()),
            "tool_search"
        );
    }

    #[test]
    fn a_long_command_is_cut_to_one_line() {
        let command = "echo ".to_string() + &"x".repeat(200);
        let label = tool_label(
            "host_shell",
            &serde_json::json!({ "command": command }).to_string(),
        );
        assert_eq!(
            label.chars().count(),
            LABEL_CAP + 1,
            "the cut plus its mark"
        );
    }

    #[test]
    fn every_terminal_state_has_its_own_line() {
        assert_eq!(compose(RunState::Reflecting, None), "Updating memory");
        assert_eq!(compose(RunState::Completed, None), "Done");
        assert_eq!(compose(RunState::Failed, None), "Failed");
        assert_eq!(compose(RunState::Canceled, None), "Stopped");
    }

    #[tokio::test]
    async fn the_opening_line_reaches_a_subscriber() {
        let hub = ProgressHub::default();
        let mut frames = hub.subscribe();
        hub.begin(
            RunId::from("run_1".to_string()),
            ChannelId::from("chn_1".to_string()),
            MessageId::from("msg_1".to_string()),
            AgentId::from("agt_1".to_string()),
            None,
            RunState::Running,
        );
        let frame = frames.try_recv().expect("the opening frame");
        assert_eq!(frame.text, "Thinking…");
        assert_eq!(frame.seq, 1);
    }

    #[tokio::test]
    async fn a_tool_call_broadcasts_the_new_line() {
        let (hub, run_id) = hub_with_run();
        let mut frames = hub.subscribe();

        hub.tool_changed(&run_id, Some("git status".to_string()));
        let frame = frames.try_recv().expect("one frame");
        assert_eq!(frame.text, "Running `git status`…");
        assert_eq!(frame.seq, 2);
        assert_eq!(frame.run_id, "run_1");
        assert_eq!(frame.channel_id, "chn_1");

        hub.tool_changed(&run_id, None);
        let frame = frames.try_recv().expect("the second frame");
        assert_eq!(frame.text, "Thinking…");
        assert_eq!(frame.seq, 3, "seq orders catch-up against live frames");
    }

    #[tokio::test]
    async fn context_compaction_has_its_own_live_line() {
        let (hub, run_id) = hub_with_run();
        let mut frames = hub.subscribe();

        hub.context_compaction_changed(&run_id, true);
        assert_eq!(
            frames.try_recv().expect("the compaction frame").text,
            "Preparing conversation…"
        );

        hub.context_compaction_changed(&run_id, false);
        assert_eq!(
            frames.try_recv().expect("the reply frame").text,
            "Thinking…"
        );
    }

    #[tokio::test]
    async fn a_state_change_broadcasts_the_new_line() {
        let (hub, run_id) = hub_with_run();
        let mut frames = hub.subscribe();

        hub.state_changed(&run_id, RunState::WaitingForApproval);
        let frame = frames.try_recv().expect("one frame");
        assert_eq!(frame.text, "Waiting for your approval");
    }

    #[test]
    fn the_snapshot_carries_the_current_line_of_its_channel() {
        let (hub, run_id) = hub_with_run();
        hub.tool_changed(&run_id, Some("git status".to_string()));

        let mine = hub.snapshot("chn_1");
        assert_eq!(mine.len(), 1);
        assert_eq!(mine[0].text, "Running `git status`…");
        assert_eq!(mine[0].seq, 2);
        assert!(hub.snapshot("chn_other").is_empty(), "gated by channel");
    }

    #[tokio::test]
    async fn the_end_broadcasts_and_returns_the_terminal_line() {
        let (hub, run_id) = hub_with_run();
        hub.tool_changed(&run_id, Some("git status".to_string()));
        let mut frames = hub.subscribe();

        let end = hub.end(&run_id, RunState::Completed).expect("live run");
        assert_eq!(end.message_id.as_str(), "msg_1");
        assert_eq!(end.channel_id.as_str(), "chn_1");
        assert_eq!(
            end.text, "Done",
            "the terminal line drops the tool in flight"
        );
        let frame = frames.try_recv().expect("the terminal frame");
        assert_eq!(frame.text, "Done", "the last frame is what the block holds");
        assert_eq!(frame.seq, 3);
        assert!(
            hub.end(&run_id, RunState::Completed).is_none(),
            "ended once"
        );
        assert!(hub.snapshot("chn_1").is_empty());
    }

    #[test]
    fn an_unknown_run_changes_nothing() {
        let hub = ProgressHub::default();
        let run_id = RunId::from("run_missing".to_string());
        hub.state_changed(&run_id, RunState::Running);
        hub.tool_changed(&run_id, Some("git status".to_string()));
        assert!(hub.snapshot("chn_1").is_empty());
    }
}
