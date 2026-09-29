//! The search index of the Software List.
//!
//! One document per package holds its name, its tool names, the
//! package and tool descriptions, its keywords and its author's name.
//! The index lives in RAM and is built whole: on daemon start, and
//! again after every publish. At the size of a Workspace's Software
//! List a rebuild costs tens of milliseconds, and a whole rebuild
//! keeps the BM25 statistics exact.
//!
//! `tantivy` is used for its tokenizer: it splits `get_weather` into
//! `get` and `weather`, so a search for "weather" finds the tool.

use tantivy::collector::TopDocs;
use tantivy::query::QueryParser;
use tantivy::schema::{STORED, Schema, TEXT, Value};
use tantivy::{Index, IndexReader, TantivyDocument};

/// The memory one index writer asks for. It is the documented floor;
/// the writer is dropped as soon as the commit lands.
const WRITER_BUDGET: usize = 15_000_000;

/// One tool as the index holds it. The description is what a search
/// answers with, so the model reads what the tool does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexedTool {
    pub name: String,
    pub description: String,
}

/// One package as the index holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexedPackage {
    pub name: String,
    pub version: String,
    pub author_name: String,
    pub description: String,
    pub keywords: Vec<String>,
    /// The tools of the latest Version, by their bare names.
    pub tools: Vec<IndexedTool>,
}

impl IndexedPackage {
    /// The bare tool names, in manifest order.
    fn tool_names(&self) -> Vec<&str> {
        self.tools.iter().map(|tool| tool.name.as_str()).collect()
    }
}

/// One package a search found.
#[derive(Debug, Clone, PartialEq)]
pub struct PackageHit {
    pub name: String,
    pub version: String,
    pub author_name: String,
    pub description: String,
    pub tools: Vec<IndexedTool>,
    pub score: f32,
}

/// The longest description a browse line carries.
const BROWSE_DESCRIPTION: usize = 120;
/// How many packages one search answers with.
const SEARCH_LIMIT: usize = 10;

pub struct ToolIndex {
    packages: Vec<IndexedPackage>,
    index: Index,
    reader: IndexReader,
    fields: Fields,
}

struct Fields {
    name: tantivy::schema::Field,
    tools: tantivy::schema::Field,
    description: tantivy::schema::Field,
    keywords: tantivy::schema::Field,
    author: tantivy::schema::Field,
    /// The position of the package in `packages`, so a hit reads the
    /// whole record and not only the indexed text.
    position: tantivy::schema::Field,
}

impl ToolIndex {
    /// Build the index over the whole Software List. An index that
    /// cannot be built is an error: the daemon logs it and offers no
    /// search rather than a wrong one.
    pub fn build(packages: Vec<IndexedPackage>) -> Result<Self, String> {
        let mut schema = Schema::builder();
        let fields = Fields {
            name: schema.add_text_field("name", TEXT),
            tools: schema.add_text_field("tools", TEXT),
            description: schema.add_text_field("description", TEXT),
            keywords: schema.add_text_field("keywords", TEXT),
            author: schema.add_text_field("author", TEXT),
            position: schema.add_u64_field("position", STORED),
        };
        let index = Index::create_in_ram(schema.build());
        {
            let mut writer = index
                .writer(WRITER_BUDGET)
                .map_err(|error| format!("cannot open the search index: {error}"))?;
            for (position, package) in packages.iter().enumerate() {
                let mut document = TantivyDocument::default();
                document.add_text(fields.name, &package.name);
                let names = package.tool_names().join(" ");
                document.add_text(fields.tools, &names);
                document.add_text(
                    fields.description,
                    format!("{} {names}", package.description),
                );
                document.add_text(fields.keywords, package.keywords.join(" "));
                document.add_text(fields.author, &package.author_name);
                document.add_u64(fields.position, position as u64);
                writer
                    .add_document(document)
                    .map_err(|error| format!("cannot index {}: {error}", package.name))?;
            }
            writer
                .commit()
                .map_err(|error| format!("cannot commit the search index: {error}"))?;
        }
        let reader = index
            .reader()
            .map_err(|error| format!("cannot read the search index: {error}"))?;
        Ok(Self {
            packages,
            index,
            reader,
            fields,
        })
    }

    /// An index over nothing. The daemon starts with one before it has
    /// read the Software List.
    pub fn empty() -> Self {
        Self::build(Vec::new()).expect("an empty index builds")
    }

    pub fn len(&self) -> usize {
        self.packages.len()
    }

    pub fn is_empty(&self) -> bool {
        self.packages.is_empty()
    }

    /// The packages that match a natural-language query, best first.
    pub fn search(&self, query: &str) -> Result<Vec<PackageHit>, String> {
        if query.trim().is_empty() {
            return Ok(Vec::new());
        }
        let fields = &self.fields;
        let parser = QueryParser::for_index(
            &self.index,
            vec![
                fields.name,
                fields.tools,
                fields.description,
                fields.keywords,
                fields.author,
            ],
        );
        let parsed = parser.parse_query_lenient(query).0;
        let searcher = self.reader.searcher();
        let found = searcher
            .search(&parsed, &TopDocs::with_limit(SEARCH_LIMIT).order_by_score())
            .map_err(|error| format!("the search failed: {error}"))?;
        let mut hits = Vec::new();
        for (score, address) in found {
            let document: TantivyDocument = searcher
                .doc(address)
                .map_err(|error| format!("the search failed: {error}"))?;
            let Some(position) = document
                .get_first(fields.position)
                .and_then(|value| value.as_u64())
            else {
                continue;
            };
            let Some(package) = self.packages.get(position as usize) else {
                continue;
            };
            hits.push(PackageHit {
                name: package.name.clone(),
                version: package.version.clone(),
                author_name: package.author_name.clone(),
                description: package.description.clone(),
                tools: package.tools.clone(),
                score,
            });
        }
        Ok(hits)
    }

    /// One line per package, by name. It is what an empty query
    /// answers with.
    pub fn browse(&self) -> Vec<String> {
        let mut packages: Vec<&IndexedPackage> = self.packages.iter().collect();
        packages.sort_by(|left, right| left.name.cmp(&right.name));
        packages.iter().map(|package| line(package)).collect()
    }
}

/// One browse line: `weather v4 by Ada: forecasts (tools: a, b)`.
fn line(package: &IndexedPackage) -> String {
    format!(
        "{} {} by {}: {} (tools: {})",
        package.name,
        package.version,
        package.author_name,
        cut(&package.description, BROWSE_DESCRIPTION),
        package.tool_names().join(", ")
    )
}

/// The first `limit` characters of a text, with an ellipsis when the
/// text is longer.
fn cut(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    let head: String = text.chars().take(limit).collect();
    format!("{}…", head.trim_end())
}
