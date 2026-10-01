//! The whole-pull percent of an image pull, from the per-layer
//! messages of the Docker pull stream. Docker announces every layer
//! with "Pulling fs layer" before it downloads one, so each layer
//! counts from the start: a small layer that finishes first does not
//! finish the pull. Each layer is one equal part, half download and
//! half extract. The containerd image store reports an extract in
//! seconds with no total, so an extract counts only when it completes.

use std::collections::HashMap;

/// How far one layer is, each part from 0 to 1.
#[derive(Debug, Default, Clone, Copy)]
struct Layer {
    download: f64,
    extract: f64,
}

#[derive(Debug, Default)]
pub(crate) struct PullProgress {
    layers: HashMap<String, Layer>,
}

impl PullProgress {
    /// Take one message of the pull stream. A message without a layer
    /// id, or with a status that is not a layer step, changes nothing.
    pub(crate) fn update(
        &mut self,
        id: Option<&str>,
        status: Option<&str>,
        current: Option<i64>,
        total: Option<i64>,
    ) {
        let (Some(id), Some(status)) = (id, status) else {
            return;
        };
        let fraction = match (current, total) {
            (Some(current), Some(total)) if total > 0 => {
                Some((current as f64 / total as f64).clamp(0.0, 1.0))
            }
            _ => None,
        };
        let step = match status {
            "Pulling fs layer" | "Waiting" => Layer::default(),
            "Downloading" => Layer {
                download: fraction.unwrap_or(0.0),
                extract: 0.0,
            },
            "Verifying Checksum" | "Download complete" => Layer {
                download: 1.0,
                extract: 0.0,
            },
            "Extracting" => Layer {
                download: 1.0,
                extract: fraction.unwrap_or(0.0),
            },
            "Pull complete" | "Already exists" => Layer {
                download: 1.0,
                extract: 1.0,
            },
            _ => return,
        };
        let layer = self.layers.entry(id.to_string()).or_default();
        // A late message never moves a layer back.
        layer.download = layer.download.max(step.download);
        layer.extract = layer.extract.max(step.extract);
    }

    /// The percent of the whole pull. It is 100 only when every layer
    /// the stream named is complete.
    pub(crate) fn percent(&self) -> u8 {
        if self.layers.is_empty() {
            return 0;
        }
        let done: f64 = self
            .layers
            .values()
            .map(|layer| (layer.download + layer.extract) / 2.0)
            .sum();
        ((done / self.layers.len() as f64) * 100.0).floor() as u8
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn announced(ids: &[&str]) -> PullProgress {
        let mut progress = PullProgress::default();
        for id in ids {
            progress.update(Some(id), Some("Pulling fs layer"), None, None);
        }
        progress
    }

    #[test]
    fn a_small_layer_that_finishes_first_does_not_finish_the_pull() {
        let mut progress = announced(&["small", "large", "larger"]);

        progress.update(Some("small"), Some("Downloading"), Some(10), Some(10));
        progress.update(Some("large"), Some("Downloading"), Some(1), Some(1000));

        assert_eq!(progress.percent(), 16);
    }

    #[test]
    fn the_pull_is_whole_when_every_layer_is_complete() {
        let mut progress = announced(&["a", "b"]);

        progress.update(Some("a"), Some("Pull complete"), None, None);
        assert_eq!(progress.percent(), 50);
        progress.update(Some("b"), Some("Already exists"), None, None);

        assert_eq!(progress.percent(), 100);
    }

    #[test]
    fn an_extract_in_seconds_counts_the_download_alone() {
        let mut progress = announced(&["a"]);

        progress.update(Some("a"), Some("Download complete"), None, None);
        progress.update(Some("a"), Some("Extracting"), Some(3), None);

        assert_eq!(progress.percent(), 50);
    }

    #[test]
    fn a_late_message_does_not_move_a_layer_back() {
        let mut progress = announced(&["a"]);

        progress.update(Some("a"), Some("Pull complete"), None, None);
        progress.update(Some("a"), Some("Downloading"), Some(5), Some(10));

        assert_eq!(progress.percent(), 100);
    }

    #[test]
    fn a_message_that_is_not_a_layer_step_is_not_a_layer() {
        let mut progress = announced(&["a"]);

        progress.update(
            Some("0.17.0"),
            Some("Pulling from pagis-co/pagis-computer"),
            None,
            None,
        );
        progress.update(None, Some("Digest: sha256:abc"), None, None);
        progress.update(Some("a"), Some("Pull complete"), None, None);

        assert_eq!(progress.percent(), 100);
    }
}
