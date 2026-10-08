//! A Run's subject comes from its trigger, before any model request.

pub enum RunTitleSource<'a> {
    Message(&'a str),
    Rule(&'a str),
    InboundCall(&'a str),
    Arrival,
    Review,
}

pub fn run_title(source: RunTitleSource<'_>) -> String {
    match source {
        RunTitleSource::Message(text) => {
            let line = text.lines().next().unwrap_or_default().trim();
            if line.is_empty() {
                return "A message with an attachment".into();
            }
            if line.chars().count() <= 80 {
                return line.into();
            }
            let head: String = line.chars().take(79).collect();
            let boundary = head.rfind(char::is_whitespace).unwrap_or(head.len());
            format!("{}…", head[..boundary].trim_end())
        }
        RunTitleSource::Rule(name) => name.into(),
        RunTitleSource::InboundCall(number) => format!("Call from {number}"),
        RunTitleSource::Arrival => "Bring a source into memory".into(),
        RunTitleSource::Review => "Review what was learned".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_trigger_names_its_run() {
        let long = "Please find a hotel near the conference and check whether breakfast and late checkout are included.";
        for (source, expected) in [
            (
                RunTitleSource::Message("Book the Austin trip"),
                "Book the Austin trip",
            ),
            (
                RunTitleSource::Message("First line\nMore detail"),
                "First line",
            ),
            (
                RunTitleSource::Message("  \nAn attachment"),
                "A message with an attachment",
            ),
            (RunTitleSource::Message(""), "A message with an attachment"),
            (
                RunTitleSource::Message(long),
                "Please find a hotel near the conference and check whether breakfast and late…",
            ),
            (
                RunTitleSource::Rule("The morning brief"),
                "The morning brief",
            ),
            (
                RunTitleSource::InboundCall("+14155550199"),
                "Call from +14155550199",
            ),
            (RunTitleSource::Arrival, "Bring a source into memory"),
            (RunTitleSource::Review, "Review what was learned"),
        ] {
            assert_eq!(run_title(source), expected);
        }
        assert_eq!(
            run_title(RunTitleSource::Message(&"界".repeat(81))),
            format!("{}…", "界".repeat(79))
        );
    }
}
