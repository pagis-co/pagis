//! The `tool_search` gate: what one call answers with,
//! and which packages it loads.

use pagis_software::{EMPTY_LIST, IndexedPackage, IndexedTool, NO_MATCH, ToolIndex, tool_search};

fn package(name: &str, description: &str, tools: &[(&str, &str)]) -> IndexedPackage {
    IndexedPackage {
        name: name.to_string(),
        version: "v1".to_string(),
        author_name: "Ada".to_string(),
        description: description.to_string(),
        keywords: Vec::new(),
        tools: tools
            .iter()
            .map(|(name, description)| IndexedTool {
                name: name.to_string(),
                description: description.to_string(),
            })
            .collect(),
    }
}

fn index() -> ToolIndex {
    ToolIndex::build(vec![
        package(
            "meteo",
            "Forecasts and severe alerts for any city.",
            &[
                ("get_weather", "The forecast of one city."),
                ("get_alerts", "The severe alerts of one city."),
            ],
        ),
        package(
            "ledger",
            "Double entry bookkeeping over a CSV file.",
            &[("post_entry", "Post one journal entry.")],
        ),
    ])
    .expect("the index builds")
}

#[test]
fn a_query_loads_every_tool_of_the_package_it_matches() {
    let outcome = tool_search(&index(), "weather forecast");

    assert_eq!(outcome.packages, ["meteo"]);
    assert_eq!(
        outcome.text,
        "meteo__get_weather: The forecast of one city.\n\
         meteo__get_alerts: The severe alerts of one city.\n\
         These tools are now callable."
    );
}

#[test]
fn a_repeat_search_answers_with_the_same_text() {
    let index = index();

    let first = tool_search(&index, "weather forecast");
    let again = tool_search(&index, "weather forecast");

    assert_eq!(first, again);
}

#[test]
fn a_query_with_no_match_says_how_to_browse() {
    let outcome = tool_search(&index(), "quantum chromodynamics");

    assert_eq!(outcome.text, NO_MATCH);
    assert!(outcome.packages.is_empty());
}

#[test]
fn an_empty_query_browses_and_loads_nothing() {
    let outcome = tool_search(&index(), "   ");

    assert_eq!(
        outcome.text,
        "ledger v1 by Ada: Double entry bookkeeping over a CSV file. (tools: post_entry)\n\
         meteo v1 by Ada: Forecasts and severe alerts for any city. (tools: get_weather, get_alerts)"
    );
    assert!(outcome.packages.is_empty());
}

#[test]
fn a_browse_of_an_empty_list_asks_for_a_publish() {
    let outcome = tool_search(&ToolIndex::empty(), "");

    assert_eq!(outcome.text, EMPTY_LIST);
}
