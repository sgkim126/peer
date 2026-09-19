use super::RenderDocument;

pub use super::hosted::{DocumentParts, summary_identity};

pub fn render(document: &RenderDocument, repo: &str) -> String {
    DocumentParts::new(document, repo).render()
}
