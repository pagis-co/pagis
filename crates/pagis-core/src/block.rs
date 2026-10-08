//! The rich message block union (ADR-0004).
//!
//! A message carries `Vec<Block>`. A `Block` is either one of the
//! typed variants, or the raw JSON of a block this build does not know.
//! The unknown arm is mandatory: a block that a newer daemon wrote
//! must survive a read by this build.
//!
//! Every variant projects to plain text. `messages.text_content` is the
//! concatenation of those projections, and it serves FTS5, the agent's
//! own context on a later turn, and another agent reading the channel.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::domain::TrustTier;

/// A table projects at most this many rows to plain text; the rest
/// become a trailing count. The full grid stays in the block.
const TABLE_TEXT_ROW_CAP: usize = 20;

/// The envelope source of a Widget's own text (ADR-0016). The
/// author of the package wrote it, so a model reads it labelled.
const WIDGET_SOURCE_PREFIX: &str = "widget:";

/// The envelope source of an inbound mail (ADR-0019): the
/// mailbox it reached. A stranger wrote the words, so a model reads
/// them labelled.
const MAIL_SOURCE_PREFIX: &str = "mail:";

/// One block of a message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Block {
    Known(KnownBlock),
    /// A block this build cannot read: an unknown `type`, or a known
    /// type whose payload does not fit. It is stored and served
    /// unchanged, and the UI renders it with the generic fallback.
    Unknown(serde_json::Value),
}

impl Block {
    pub fn markdown(text: impl Into<String>) -> Self {
        KnownBlock::Markdown { text: text.into() }.into()
    }

    pub fn image(artifact_id: impl Into<String>, alt: Option<String>) -> Self {
        KnownBlock::Image {
            artifact_id: artifact_id.into(),
            alt,
        }
        .into()
    }

    pub fn file(
        artifact_id: impl Into<String>,
        name: impl Into<String>,
        mime: Option<String>,
        size_bytes: Option<i64>,
    ) -> Self {
        KnownBlock::File {
            artifact_id: artifact_id.into(),
            name: name.into(),
            mime,
            size_bytes,
        }
        .into()
    }

    pub fn approval_card(
        request_id: impl Into<String>,
        title: impl Into<String>,
        body: impl Into<String>,
    ) -> Self {
        KnownBlock::ApprovalCard {
            request_id: request_id.into(),
            title: title.into(),
            body: body.into(),
        }
        .into()
    }

    /// The view of a `form` Request. The daemon mints it from the row
    /// it wrote; `ask_user` supplies the content (ADR-0004).
    pub fn form(
        request_id: impl Into<String>,
        title: impl Into<String>,
        fields: Vec<FormField>,
        submit_label: Option<String>,
    ) -> Self {
        KnownBlock::Form {
            request_id: request_id.into(),
            title: title.into(),
            fields,
            submit_label,
        }
        .into()
    }

    /// The view of a `choice` Request (ADR-0004).
    pub fn choice_card(
        request_id: impl Into<String>,
        title: impl Into<String>,
        body: Option<String>,
        options: Vec<ChoiceOption>,
    ) -> Self {
        KnownBlock::ChoiceCard {
            request_id: request_id.into(),
            title: title.into(),
            body,
            options,
        }
        .into()
    }

    /// The view of one Widget a tool result rendered (ADR-0016).
    /// Daemon-minted: it names a Run, a resolved Version and a tool
    /// call, which are rows the daemon owns.
    pub fn widget(
        package: impl Into<String>,
        version: impl Into<String>,
        widget: impl Into<String>,
        tool_call_id: impl Into<String>,
        text: impl Into<String>,
        request_id: Option<String>,
    ) -> Self {
        KnownBlock::Widget {
            package: package.into(),
            version: version.into(),
            widget: widget.into(),
            tool_call_id: tool_call_id.into(),
            text: text.into(),
            request_id,
        }
        .into()
    }

    /// The view of one mail: an inbound mail that woke the Agent, or
    /// a mail the Agent sent (ADR-0019). Daemon-minted: it names
    /// a mailbox the daemon owns, and no tool writes it.
    pub fn mail(
        direction: MailDirection,
        mailbox: impl Into<String>,
        message_id: impl Into<String>,
        counterpart: impl Into<String>,
        subject: impl Into<String>,
        trust_tier: Option<TrustTier>,
    ) -> Self {
        KnownBlock::Mail {
            direction,
            mailbox: mailbox.into(),
            message_id: message_id.into(),
            counterpart: counterpart.into(),
            subject: subject.into(),
            trust_tier,
        }
        .into()
    }

    /// The view of one Coding Session in its Thread (ADR-0033).
    /// Daemon-minted: it names a record the daemon owns. It carries
    /// copies of four display fields; the live state comes from the
    /// record.
    pub fn coding_session(
        coding_session_id: impl Into<String>,
        harness: impl Into<String>,
        machine: impl Into<String>,
        directory: impl Into<String>,
        title: impl Into<String>,
    ) -> Self {
        KnownBlock::CodingSession {
            coding_session_id: coding_session_id.into(),
            harness: harness.into(),
            machine: machine.into(),
            directory: directory.into(),
            title: title.into(),
        }
        .into()
    }

    /// The derived run progress line. The daemon composes the text;
    /// no tool writes it (ADR-0004).
    pub fn progress(run_id: impl Into<String>, text: impl Into<String>) -> Self {
        KnownBlock::Progress {
            run_id: run_id.into(),
            text: text.into(),
        }
        .into()
    }

    /// Whether this block is a derived progress line. A message that
    /// carries one is daemon-derived state, not conversation.
    pub fn is_progress(&self) -> bool {
        matches!(self, Block::Known(KnownBlock::Progress { .. }))
    }

    /// The plain-text projection of this block.
    pub fn text_projection(&self) -> String {
        match self {
            Block::Known(block) => block.text_projection(),
            // An unreadable block still contributes its text when it
            // carries one, which is what the UI fallback shows too.
            Block::Unknown(value) => value
                .get("text")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string(),
        }
    }
}

impl From<KnownBlock> for Block {
    fn from(block: KnownBlock) -> Self {
        Block::Known(block)
    }
}

/// The plain-text projection of a whole block array: every non-empty
/// block projection, in order, separated by a blank line.
pub fn blocks_text(blocks: &[Block]) -> String {
    blocks
        .iter()
        .map(Block::text_projection)
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// The block types the daemon and the UI both know.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum KnownBlock {
    /// Prose. An agent's own text becomes markdown blocks.
    Markdown { text: String },
    /// A declarative grid: no server pagination, no sort configuration.
    Table {
        columns: Vec<TableColumn>,
        #[serde(default)]
        rows: Vec<Vec<TableCell>>,
    },
    /// The view of a `form` Request: fill then submit. Live state and
    /// the submitted values come from the Request row.
    Form {
        request_id: String,
        title: String,
        fields: Vec<FormField>,
        #[serde(default)]
        submit_label: Option<String>,
    },
    /// The view of a `choice` Request: one tap.
    ChoiceCard {
        request_id: String,
        title: String,
        #[serde(default)]
        body: Option<String>,
        options: Vec<ChoiceOption>,
    },
    /// The view of a `tool_action` Request. Daemon-minted: an agent
    /// that could mint one would forge authority over its own gate.
    ApprovalCard {
        request_id: String,
        title: String,
        body: String,
    },
    /// Derived run progress. The daemon composes the text; there is no
    /// tool that writes it.
    Progress { run_id: String, text: String },
    /// An image artifact, fetched through the daemon.
    Image {
        artifact_id: String,
        #[serde(default)]
        alt: Option<String>,
    },
    /// A non-image artifact, downloaded through the daemon.
    File {
        artifact_id: String,
        name: String,
        #[serde(default)]
        mime: Option<String>,
        #[serde(default)]
        size_bytes: Option<i64>,
    },
    /// A live view of one agent's computer screen.
    Screen { agent_id: String },
    /// A call, live or over (ADR-0020). While the call runs, the
    /// block listens to it.
    Call { call_id: String },
    /// One Widget of a Software Package, rendered in a sandboxed frame
    /// (ADR-0016). It carries no HTML and no data: the UI reads
    /// the page by package, version and Widget name, and the data by
    /// tool call id. `request_id` is set when the Widget asks the user
    /// and the Run parks on the answer.
    Widget {
        package: String,
        version: String,
        widget: String,
        tool_call_id: String,
        /// The author's plain-text projection of what the Widget
        /// shows. It is foreign text, so the projection labels it.
        text: String,
        #[serde(default)]
        request_id: Option<String>,
    },
    /// One mail, inbound or sent (ADR-0019). It carries the
    /// envelope and never the words: the inspector reads the headers
    /// and the text body live through the daemon, which stores none of
    /// them.
    Mail {
        direction: MailDirection,
        /// The mailbox the message reached: the Agent's own address,
        /// or the alias of one of the user's accounts. It labels the
        /// line, and the inspector reads the message through it.
        mailbox: String,
        /// The `folder:uid` id `mail__get_message` takes.
        message_id: String,
        /// The other party: who sent an inbound mail, and who a sent
        /// mail went to. Foreign text on an inbound mail.
        counterpart: String,
        /// The subject. Foreign text on an inbound mail.
        subject: String,
        /// The tier of the sender, stamped before the Agent read a
        /// word (ADR-0019). A mail the Agent sent carries none.
        #[serde(default)]
        trust_tier: Option<TrustTier>,
    },
    /// One Coding Session (ADR-0033). The state, the usage and the
    /// pending decision come from the record; the block carries copies
    /// of its display fields.
    CodingSession {
        coding_session_id: String,
        /// The display name of the harness in the Harness Catalog.
        harness: String,
        /// The name of the Host, or "Computer" for a session in the
        /// Agent's Computer.
        machine: String,
        /// The directory that the Agent named.
        directory: String,
        /// The title that the Agent wrote.
        title: String,
    },
}

impl KnownBlock {
    /// Whether an agent may append this block to its own message with
    /// `add_block` (ADR-0004). Content is true; a view of a durable
    /// row the daemon owns is false, and so is a block that asks,
    /// which is `ask_user`'s job.
    pub fn is_content(&self) -> bool {
        matches!(
            self,
            KnownBlock::Markdown { .. }
                | KnownBlock::Table { .. }
                | KnownBlock::Image { .. }
                | KnownBlock::File { .. }
        )
    }

    fn text_projection(&self) -> String {
        match self {
            KnownBlock::Markdown { text } => text.clone(),
            KnownBlock::Table { columns, rows } => table_text(columns, rows),
            KnownBlock::Form { title, fields, .. } => {
                let mut out = title.clone();
                for field in fields {
                    out.push_str(&format!("\n- {}", field.label));
                }
                out
            }
            KnownBlock::ChoiceCard {
                title,
                body,
                options,
                ..
            } => {
                let mut out = title.clone();
                if let Some(body) = body {
                    out.push_str(&format!("\n{body}"));
                }
                for option in options {
                    out.push_str(&format!("\n- {}", option.label));
                }
                out
            }
            KnownBlock::ApprovalCard { title, body, .. } => format!("{title}\n{body}"),
            KnownBlock::Progress { text, .. } => text.clone(),
            KnownBlock::Image { alt, .. } => alt.clone().unwrap_or_else(|| "image".to_string()),
            KnownBlock::File { name, .. } => name.clone(),
            KnownBlock::Screen { .. } => "[live screen]".to_string(),
            KnownBlock::Call { .. } => "[call]".to_string(),
            KnownBlock::Widget {
                package,
                widget,
                text,
                ..
            } => crate::untrusted::wrap(&format!("{WIDGET_SOURCE_PREFIX}{package}/{widget}"), text),
            KnownBlock::Mail {
                direction,
                mailbox,
                counterpart,
                subject,
                trust_tier,
                ..
            } => mail_text(*direction, mailbox, counterpart, subject, *trust_tier),
            // The Agent wrote the title and the daemon the rest. No part
            // is harness output, so the line needs no envelope.
            KnownBlock::CodingSession {
                harness,
                machine,
                directory,
                title,
                ..
            } => format!("Coding session \"{title}\": {harness} on {machine} in {directory}"),
        }
    }
}

/// Which way one mail went (ADR-0019): into the mailbox, or out of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum MailDirection {
    Inbound,
    Outbound,
}

/// The line one mail projects. An inbound mail is words a stranger
/// wrote, so it goes inside the untrusted envelope with the tier of
/// its sender (ADR-0019). A mail the Agent sent is the Agent's
/// own words and needs no envelope.
fn mail_text(
    direction: MailDirection,
    mailbox: &str,
    counterpart: &str,
    subject: &str,
    trust_tier: Option<TrustTier>,
) -> String {
    match direction {
        MailDirection::Inbound => {
            let tier = trust_tier.unwrap_or(TrustTier::Unknown);
            crate::untrusted::Untrusted::text(
                format!("{MAIL_SOURCE_PREFIX}{mailbox}"),
                tier.as_str(),
                &format!("mail from {counterpart}: {subject}"),
            )
            .content
        }
        MailDirection::Outbound => format!("mail to {counterpart}: {subject}"),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum TableAlign {
    Left,
    Center,
    Right,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct TableColumn {
    /// The column's stable key; the UI sorts on it.
    pub key: String,
    pub label: String,
    #[serde(default)]
    pub align: Option<TableAlign>,
}

/// One table cell: a closed union, so a cell carries no markup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TableCell {
    Text {
        text: String,
    },
    Number {
        #[schema(value_type = f64)]
        number: serde_json::Number,
    },
    Link {
        href: String,
        #[serde(default)]
        label: Option<String>,
    },
    Timestamp {
        unix_ms: i64,
    },
}

impl TableCell {
    fn text_projection(&self) -> String {
        match self {
            TableCell::Text { text } => text.clone(),
            TableCell::Number { number } => number.to_string(),
            TableCell::Link { href, label } => match label {
                Some(label) => format!("[{label}]({href})"),
                None => href.clone(),
            },
            TableCell::Timestamp { unix_ms } => format_unix_ms(*unix_ms),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum FormFieldKind {
    Text,
    Number,
    Select,
    Checkbox,
    Date,
}

/// One flat form field. There is no nesting, no conditional
/// visibility, and no layout hint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct FormField {
    /// The key the submitted value lands under.
    pub key: String,
    pub label: String,
    pub kind: FormFieldKind,
    #[serde(default)]
    pub required: bool,
    /// The choices of a `select` field; empty for every other kind.
    #[serde(default)]
    pub options: Vec<ChoiceOption>,
    #[serde(default)]
    pub placeholder: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ChoiceOption {
    /// The value the decision carries back to the run.
    pub value: String,
    pub label: String,
    #[serde(default)]
    pub description: Option<String>,
}

/// A table as a markdown grid, capped at `TABLE_TEXT_ROW_CAP` rows.
fn table_text(columns: &[TableColumn], rows: &[Vec<TableCell>]) -> String {
    let header: Vec<String> = columns.iter().map(|c| escape_cell(&c.label)).collect();
    let rule: Vec<&str> = columns
        .iter()
        .map(|c| match c.align {
            Some(TableAlign::Left) => ":---",
            Some(TableAlign::Center) => ":---:",
            Some(TableAlign::Right) => "---:",
            None => "---",
        })
        .collect();
    let mut out = format!("| {} |\n| {} |", header.join(" | "), rule.join(" | "));
    for row in rows.iter().take(TABLE_TEXT_ROW_CAP) {
        let cells: Vec<String> = row
            .iter()
            .map(|cell| escape_cell(&cell.text_projection()))
            .collect();
        out.push_str(&format!("\n| {} |", cells.join(" | ")));
    }
    if rows.len() > TABLE_TEXT_ROW_CAP {
        out.push_str(&format!(
            "\n… and {} more rows",
            rows.len() - TABLE_TEXT_ROW_CAP
        ));
    }
    out
}

/// Keep one cell on one markdown row.
fn escape_cell(text: &str) -> String {
    text.replace('|', "\\|").replace(['\n', '\r'], " ")
}

/// `YYYY-MM-DDTHH:MM:SSZ` for one instant, so a timestamp cell reads
/// as a date to a model and not as an integer.
fn format_unix_ms(unix_ms: i64) -> String {
    let seconds = unix_ms.div_euclid(1000);
    let days = seconds.div_euclid(86_400);
    let time_of_day = seconds.rem_euclid(86_400);
    let (hour, minute, second) = (
        time_of_day / 3600,
        (time_of_day % 3600) / 60,
        time_of_day % 60,
    );
    // Civil date from a day count, shifted to a March-based year so
    // the leap day lands last.
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let march_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * march_month + 2) / 5 + 1;
    let month = if march_month < 10 {
        march_month + 3
    } else {
        march_month - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn every_variant() -> Vec<Block> {
        vec![
            Block::markdown("hello"),
            KnownBlock::Table {
                columns: vec![
                    TableColumn {
                        key: "name".to_string(),
                        label: "Name".to_string(),
                        align: None,
                    },
                    TableColumn {
                        key: "count".to_string(),
                        label: "Count".to_string(),
                        align: Some(TableAlign::Right),
                    },
                ],
                rows: vec![vec![
                    TableCell::Text {
                        text: "widgets".to_string(),
                    },
                    TableCell::Number {
                        number: serde_json::Number::from(3),
                    },
                ]],
            }
            .into(),
            KnownBlock::Form {
                request_id: "req_1".to_string(),
                title: "Book the room".to_string(),
                fields: vec![FormField {
                    key: "when".to_string(),
                    label: "When".to_string(),
                    kind: FormFieldKind::Date,
                    required: true,
                    options: Vec::new(),
                    placeholder: None,
                }],
                submit_label: Some("Book".to_string()),
            }
            .into(),
            KnownBlock::ChoiceCard {
                request_id: "req_2".to_string(),
                title: "Which date?".to_string(),
                body: Some("Both work for me.".to_string()),
                options: vec![ChoiceOption {
                    value: "tue".to_string(),
                    label: "Tuesday".to_string(),
                    description: None,
                }],
            }
            .into(),
            Block::approval_card("apr_1", "Run a command", "echo hi"),
            KnownBlock::Progress {
                run_id: "run_1".to_string(),
                text: "Reading the calendar".to_string(),
            }
            .into(),
            Block::image("art_1", Some("a red square".to_string())),
            Block::file(
                "art_2",
                "notes.txt",
                Some("text/plain".to_string()),
                Some(12),
            ),
            KnownBlock::Screen {
                agent_id: "agt_1".to_string(),
            }
            .into(),
        ]
    }

    #[test]
    fn every_variant_round_trips_through_json() {
        let blocks = every_variant();
        assert_eq!(blocks.len(), 9, "the vocabulary is nine types");
        let json = serde_json::to_value(&blocks).unwrap();
        let back: Vec<Block> = serde_json::from_value(json).unwrap();
        assert_eq!(back, blocks);
    }

    #[test]
    fn each_variant_carries_its_type_tag() {
        let json = serde_json::to_value(every_variant()).unwrap();
        let types: Vec<&str> = json
            .as_array()
            .unwrap()
            .iter()
            .map(|b| b["type"].as_str().unwrap())
            .collect();
        assert_eq!(
            types,
            [
                "markdown",
                "table",
                "form",
                "choice_card",
                "approval_card",
                "progress",
                "image",
                "file",
                "screen",
            ]
        );
    }

    #[test]
    fn only_content_blocks_are_agent_emittable() {
        let content: Vec<bool> = every_variant()
            .iter()
            .map(|block| match block {
                Block::Known(known) => known.is_content(),
                Block::Unknown(_) => false,
            })
            .collect();
        // markdown, table, image and file are content; the Request
        // views and the daemon-owned blocks are not (ADR-0004).
        assert_eq!(
            content,
            [true, true, false, false, false, false, true, true, false]
        );
    }

    #[test]
    fn an_unknown_type_survives_unchanged() {
        let raw = serde_json::json!({"type": "hologram", "text": "from a newer daemon"});
        let block: Block = serde_json::from_value(raw.clone()).unwrap();
        assert_eq!(block, Block::Unknown(raw.clone()));
        assert_eq!(serde_json::to_value(&block).unwrap(), raw);
    }

    #[test]
    fn a_known_type_with_a_broken_payload_falls_back() {
        let raw = serde_json::json!({"type": "markdown", "text": 7});
        let block: Block = serde_json::from_value(raw.clone()).unwrap();
        assert_eq!(block, Block::Unknown(raw));
    }

    #[test]
    fn an_unknown_block_projects_its_text() {
        let block: Block =
            serde_json::from_value(serde_json::json!({"type": "hologram", "text": "hi"})).unwrap();
        assert_eq!(block.text_projection(), "hi");
    }

    #[test]
    fn each_variant_projects_to_plain_text() {
        let projections: Vec<String> = every_variant().iter().map(Block::text_projection).collect();
        assert_eq!(projections[0], "hello");
        assert_eq!(
            projections[1],
            "| Name | Count |\n| --- | ---: |\n| widgets | 3 |"
        );
        assert_eq!(projections[2], "Book the room\n- When");
        assert_eq!(projections[3], "Which date?\nBoth work for me.\n- Tuesday");
        assert_eq!(projections[4], "Run a command\necho hi");
        assert_eq!(projections[5], "Reading the calendar");
        assert_eq!(projections[6], "a red square");
        assert_eq!(projections[7], "notes.txt");
        assert_eq!(projections[8], "[live screen]");
    }

    #[test]
    fn an_image_without_alt_projects_a_placeholder() {
        assert_eq!(Block::image("art_1", None).text_projection(), "image");
    }

    #[test]
    fn a_long_table_projects_a_capped_grid() {
        let rows: Vec<Vec<TableCell>> = (0..TABLE_TEXT_ROW_CAP + 5)
            .map(|i| {
                vec![TableCell::Text {
                    text: format!("row {i}"),
                }]
            })
            .collect();
        let block: Block = KnownBlock::Table {
            columns: vec![TableColumn {
                key: "r".to_string(),
                label: "R".to_string(),
                align: None,
            }],
            rows,
        }
        .into();
        let text = block.text_projection();
        assert!(text.contains("| row 19 |"));
        assert!(!text.contains("| row 20 |"));
        assert!(text.ends_with("… and 5 more rows"));
    }

    #[test]
    fn a_cell_stays_on_one_row() {
        let block: Block = KnownBlock::Table {
            columns: vec![TableColumn {
                key: "r".to_string(),
                label: "R".to_string(),
                align: None,
            }],
            rows: vec![vec![TableCell::Text {
                text: "a | b\nc".to_string(),
            }]],
        }
        .into();
        assert!(block.text_projection().ends_with("| a \\| b c |"));
    }

    #[test]
    fn a_link_and_a_timestamp_cell_read_as_text() {
        assert_eq!(
            TableCell::Link {
                href: "https://example.com".to_string(),
                label: Some("here".to_string()),
            }
            .text_projection(),
            "[here](https://example.com)"
        );
        assert_eq!(
            TableCell::Timestamp { unix_ms: 0 }.text_projection(),
            "1970-01-01T00:00:00Z"
        );
        assert_eq!(
            TableCell::Timestamp {
                unix_ms: 1_756_468_800_000
            }
            .text_projection(),
            "2025-08-29T12:00:00Z"
        );
    }

    /// The Widget block (ADR-0016).
    fn widget_block(request_id: Option<String>) -> Block {
        Block::widget(
            "weather",
            "v0.1.0",
            "chart",
            "call_1",
            "Tomorrow reaches 21 degrees.",
            request_id,
        )
    }

    #[test]
    fn a_widget_block_names_a_version_and_a_tool_call() {
        let json = serde_json::to_value(widget_block(None)).unwrap();

        assert_eq!(json["type"], "widget");
        assert_eq!(json["package"], "weather");
        assert_eq!(json["version"], "v0.1.0");
        assert_eq!(json["widget"], "chart");
        assert_eq!(json["tool_call_id"], "call_1");
        assert!(json["request_id"].is_null());
        // It carries neither the page nor the data.
        assert!(json.get("html").is_none());
        assert!(json.get("structured_content").is_none());
    }

    #[test]
    fn a_widget_block_that_asks_names_its_request() {
        let json = serde_json::to_value(widget_block(Some("req_9".to_string()))).unwrap();

        assert_eq!(json["request_id"], "req_9");
    }

    #[test]
    fn a_widget_block_round_trips_through_json() {
        let block = widget_block(Some("req_9".to_string()));
        let json = serde_json::to_value(&block).unwrap();

        assert_eq!(serde_json::from_value::<Block>(json).unwrap(), block);
    }

    #[test]
    fn a_widget_projects_the_author_text_labelled_untrusted() {
        let text = widget_block(None).text_projection();

        assert!(text.contains("Tomorrow reaches 21 degrees."), "{text}");
        assert!(text.contains("widget:weather/chart"), "{text}");
        assert!(text.starts_with("[BEGIN UNTRUSTED"), "{text}");
    }

    #[test]
    fn an_agent_cannot_emit_a_widget_block() {
        let Block::Known(known) = widget_block(None) else {
            panic!("a known block");
        };
        assert!(!known.is_content());
    }

    /// The mail block (ADR-0019).
    fn inbound_mail() -> Block {
        Block::mail(
            MailDirection::Inbound,
            "ada@example.com",
            "INBOX:1234",
            "Clinic <care@clinic.test>",
            "Your appointment",
            Some(TrustTier::Trusted),
        )
    }

    #[test]
    fn a_mail_block_carries_the_envelope_and_no_words() {
        let json = serde_json::to_value(inbound_mail()).unwrap();

        assert_eq!(json["type"], "mail");
        assert_eq!(json["direction"], "inbound");
        assert_eq!(json["mailbox"], "ada@example.com");
        assert_eq!(json["message_id"], "INBOX:1234");
        assert_eq!(json["counterpart"], "Clinic <care@clinic.test>");
        assert_eq!(json["subject"], "Your appointment");
        assert_eq!(json["trust_tier"], "trusted");
        assert!(json.get("body").is_none());
        assert!(json.get("snippet").is_none());
    }

    #[test]
    fn a_mail_block_round_trips_through_json() {
        let block = inbound_mail();
        let json = serde_json::to_value(&block).unwrap();

        assert_eq!(serde_json::from_value::<Block>(json).unwrap(), block);
    }

    #[test]
    fn an_inbound_mail_projects_inside_the_untrusted_envelope() {
        let text = inbound_mail().text_projection();

        assert!(text.starts_with("[BEGIN UNTRUSTED"), "{text}");
        assert!(text.contains("source=mail:ada@example.com"), "{text}");
        assert!(text.contains("trust=trusted"), "{text}");
        assert!(
            text.contains("mail from Clinic <care@clinic.test>: Your appointment"),
            "{text}"
        );
    }

    #[test]
    fn a_mail_with_no_tier_projects_as_unknown() {
        let text = Block::mail(
            MailDirection::Inbound,
            "ada@example.com",
            "INBOX:2",
            "someone@example.test",
            "Hello",
            None,
        )
        .text_projection();

        assert!(text.contains("trust=unknown"), "{text}");
    }

    #[test]
    fn a_sent_mail_projects_its_own_words_without_an_envelope() {
        let text = Block::mail(
            MailDirection::Outbound,
            "ada@example.com",
            "<sent-1@example.com>",
            "care@clinic.test",
            "The appointment",
            None,
        )
        .text_projection();

        assert_eq!(text, "mail to care@clinic.test: The appointment");
    }

    #[test]
    fn an_agent_cannot_emit_a_mail_block() {
        let Block::Known(known) = inbound_mail() else {
            panic!("a known block");
        };
        assert!(!known.is_content());
    }

    /// The Coding Session block (ADR-0033).
    fn coding_session_block() -> Block {
        Block::coding_session(
            "cs_1",
            "Claude Code",
            "Air",
            "/Users/bo/code/app",
            "Fix the login bug",
        )
    }

    #[test]
    fn a_coding_session_block_round_trips_through_json() {
        let block = coding_session_block();
        let json = serde_json::to_value(&block).unwrap();

        assert_eq!(
            json,
            serde_json::json!({
                "type": "coding_session",
                "coding_session_id": "cs_1",
                "harness": "Claude Code",
                "machine": "Air",
                "directory": "/Users/bo/code/app",
                "title": "Fix the login bug",
            })
        );
        assert_eq!(serde_json::from_value::<Block>(json).unwrap(), block);
    }

    #[test]
    fn a_coding_session_block_projects_to_one_line_with_no_envelope() {
        assert_eq!(
            coding_session_block().text_projection(),
            "Coding session \"Fix the login bug\": Claude Code on Air in /Users/bo/code/app"
        );
    }

    #[test]
    fn an_agent_cannot_emit_a_coding_session_block() {
        let Block::Known(known) = coding_session_block() else {
            panic!("a known block");
        };
        assert!(!known.is_content());
    }

    #[test]
    fn the_message_projection_joins_the_blocks() {
        let blocks = vec![
            Block::markdown("Here it is."),
            Block::file("art_1", "notes.txt", None, None),
            Block::markdown("   "),
        ];
        assert_eq!(blocks_text(&blocks), "Here it is.\n\nnotes.txt");
    }
}
