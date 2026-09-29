use super::SourceKey;

/// A committed knowledge change waiting for publication. Contains no source content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnowledgeInvalidation {
    pub id: i64,
    pub source: SourceKey,
}
