//! Recall of the Software List index.
//!
//! The fixture is a Workspace with a dozen packages. Each query names
//! the package it must find in the top three. The table holds the
//! three shapes the search must serve: a word inside a `snake_case`
//! tool name, a keyword the description never says, and an author's
//! name.

use pagis_software::{IndexedPackage, IndexedTool, ToolIndex};

fn package(
    name: &str,
    author: &str,
    description: &str,
    keywords: &[&str],
    tools: &[&str],
) -> IndexedPackage {
    IndexedPackage {
        name: name.to_string(),
        version: "v1".to_string(),
        author_name: author.to_string(),
        description: description.to_string(),
        keywords: keywords.iter().map(|word| word.to_string()).collect(),
        tools: tools.iter().map(|name| tool(name)).collect(),
    }
}

/// One tool of a fixture package. Recall is about the name, so every
/// fixture tool describes itself by its name.
fn tool(name: &str) -> IndexedTool {
    IndexedTool {
        name: name.to_string(),
        description: format!("the {name} tool"),
    }
}

fn fixture() -> Vec<IndexedPackage> {
    vec![
        package(
            "meteo",
            "Ada",
            "Forecasts and severe alerts for any city.",
            &["climate"],
            &["get_weather", "get_alerts"],
        ),
        package(
            "ledger",
            "Ada",
            "Double entry bookkeeping over a CSV file.",
            &["accounting", "invoice"],
            &["post_entry", "trial_balance"],
        ),
        package(
            "postmark",
            "Bo",
            "Send transactional mail and read the delivery state.",
            &["email"],
            &["send_mail", "delivery_state"],
        ),
        package(
            "cartograph",
            "Bo",
            "Draw a map from a list of coordinates.",
            &["geo", "map"],
            &["render_map", "geocode"],
        ),
        package(
            "lexicon",
            "Cyd",
            "Look a word up and give its senses.",
            &["dictionary"],
            &["define_word", "synonyms"],
        ),
        package(
            "photon",
            "Cyd",
            "Resize, crop and convert pictures.",
            &["image"],
            &["resize_image", "convert_image"],
        ),
        package(
            "chronos",
            "Dov",
            "Convert times between zones and format them.",
            &["time", "timezone"],
            &["convert_time", "format_time"],
        ),
        package(
            "quill",
            "Dov",
            "Turn markdown into clean printable pages.",
            &["markdown", "pdf"],
            &["render_pdf", "outline"],
        ),
        package(
            "sifter",
            "Eir",
            "Filter and summarize a long table of rows.",
            &["csv", "table"],
            &["filter_rows", "summarize_rows"],
        ),
        package(
            "beacon",
            "Eir",
            "Watch a web page and report what changed.",
            &["monitor"],
            &["watch_page", "diff_page"],
        ),
        package(
            "abacus",
            "Fen",
            "Evaluate arithmetic and unit conversions.",
            &["math", "units"],
            &["evaluate", "convert_unit"],
        ),
        package(
            "scribe",
            "Fen",
            "Write and read notes in a plain folder.",
            &["notes"],
            &["write_note", "read_note"],
        ),
    ]
}

/// Each query and the package it must find in the top three.
const RECALL: &[(&str, &str)] = &[
    // A word inside a `snake_case` tool name, which the package name
    // and the description never say.
    ("weather", "meteo"),
    // A keyword only.
    ("invoice", "ledger"),
    // An author's name.
    ("Cyd", "lexicon"),
    ("resize a photo", "photon"),
    ("timezone conversion", "chronos"),
    ("send an email", "postmark"),
    ("map coordinates", "cartograph"),
    ("summarize a csv table", "sifter"),
    ("watch a page for changes", "beacon"),
    ("unit conversion math", "abacus"),
    ("take notes", "scribe"),
    ("markdown to pdf", "quill"),
];

#[test]
fn every_query_finds_its_package_in_the_top_three() {
    let index = ToolIndex::build(fixture()).expect("the index builds");

    for (query, expected) in RECALL {
        let names: Vec<String> = index
            .search(query)
            .expect("the search runs")
            .into_iter()
            .take(3)
            .map(|hit| hit.name)
            .collect();
        assert!(
            names.iter().any(|name| name == expected),
            "{query:?} must find {expected:?}; it found {names:?}"
        );
    }
}

#[test]
fn browse_gives_one_line_per_package_by_name() {
    let index = ToolIndex::build(fixture()).expect("the index builds");

    let lines = index.browse();

    assert_eq!(lines.len(), 12);
    assert_eq!(
        lines[0],
        "abacus v1 by Fen: Evaluate arithmetic and unit conversions. (tools: evaluate, convert_unit)"
    );
    let names: Vec<&str> = lines
        .iter()
        .map(|line| line.split(' ').next().expect("a name"))
        .collect();
    let mut sorted = names.clone();
    sorted.sort_unstable();
    assert_eq!(names, sorted);
}

#[test]
fn a_long_description_is_cut_in_the_browse_line() {
    let long = "x".repeat(200);
    let index = ToolIndex::build(vec![IndexedPackage {
        name: "verbose".to_string(),
        version: "v2".to_string(),
        author_name: "Ada".to_string(),
        description: long,
        keywords: Vec::new(),
        tools: vec![tool("one")],
    }])
    .expect("the index builds");

    let line = index.browse().remove(0);

    assert_eq!(
        line,
        format!("verbose v2 by Ada: {}… (tools: one)", "x".repeat(120))
    );
}

#[test]
fn an_empty_query_finds_nothing() {
    let index = ToolIndex::build(fixture()).expect("the index builds");

    assert_eq!(index.search("   ").expect("the search runs"), Vec::new());
}

// --- Reachability ---
//
// Recall is about the order of the hits; reachability is the floor
// under it: a search must never hide a package that exists. Every
// package of the fixture and 200 more must answer to its own name, to
// each of its tool names and to each word of a tool name, and the
// browse must list each one once.

/// How many packages the generated part of the list holds.
const GENERATED: usize = 200;
/// How many hits a reachability query looks through. It is how many
/// packages one `tool_search` call loads.
const TOP: usize = 5;

/// One made-up word for a package or tool name. The map from a number
/// to a word is injective, so every generated word is its own and no
/// query speaks for two packages.
fn word(number: usize) -> String {
    const CONSONANTS: [char; 20] = [
        'b', 'c', 'd', 'f', 'g', 'h', 'j', 'k', 'l', 'm', 'n', 'p', 'q', 'r', 's', 't', 'v', 'w',
        'x', 'z',
    ];
    const VOWELS: [char; 5] = ['a', 'e', 'i', 'o', 'u'];
    let mut name = String::new();
    name.push(CONSONANTS[number % 20]);
    name.push(VOWELS[(number / 20) % 5]);
    name.push(CONSONANTS[(number / 100) % 20]);
    name.push(VOWELS[(number / 2000) % 5]);
    name.push(CONSONANTS[(number / 10_000) % 20]);
    name
}

/// The word at one position of the generated list. The order is its
/// own, and it is the same on every run: a stored test must say the
/// same thing today and tomorrow. The step and the modulus are prime
/// to each other, so no number comes twice.
fn scrambled(position: usize) -> String {
    word((position * 7919 + 13) % 100_003)
}

/// The fixture and 200 generated packages, each with three tools of
/// two words.
fn crowd() -> Vec<IndexedPackage> {
    let mut packages = fixture();
    for index in 0..GENERATED {
        let base = index * 7;
        let tools: Vec<IndexedTool> = (0..3)
            .map(|number| {
                tool(&format!(
                    "{}_{}",
                    scrambled(base + 1 + number * 2),
                    scrambled(base + 2 + number * 2)
                ))
            })
            .collect();
        packages.push(IndexedPackage {
            name: scrambled(base),
            version: "v1".to_string(),
            author_name: "Ada".to_string(),
            description: "A generated package of the reachability test.".to_string(),
            keywords: Vec::new(),
            tools,
        });
    }
    packages
}

/// The queries one package must answer to: its name, each tool name,
/// and each word of a tool name.
fn queries(package: &IndexedPackage) -> Vec<String> {
    let mut queries = vec![package.name.clone()];
    for tool in &package.tools {
        queries.push(tool.name.clone());
        queries.extend(tool.name.split('_').map(|word| word.to_string()));
    }
    queries
}

#[test]
fn every_package_answers_to_its_name_and_to_every_tool_word() {
    let packages = crowd();
    let index = ToolIndex::build(packages.clone()).expect("the index builds");

    for package in &packages {
        for query in queries(package) {
            let names: Vec<String> = index
                .search(&query)
                .expect("the search runs")
                .into_iter()
                .take(TOP)
                .map(|hit| hit.name)
                .collect();
            assert!(
                names.contains(&package.name),
                "{query:?} must find {:?} in the top {TOP}; it found {names:?}",
                package.name
            );
        }
    }
}

#[test]
fn browse_lists_every_package_of_a_crowded_list_once() {
    let packages = crowd();
    let index = ToolIndex::build(packages.clone()).expect("the index builds");

    let lines = index.browse();

    assert_eq!(lines.len(), packages.len());
    for package in &packages {
        let found: Vec<&String> = lines
            .iter()
            .filter(|line| line.starts_with(&format!("{} v1 by ", package.name)))
            .collect();
        assert_eq!(found.len(), 1, "{} is listed {found:?}", package.name);
    }
}
