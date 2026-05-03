use aml::prelude::Document;

pub fn render(markup: &str) -> String {
    Document::new(markup).render()
}
