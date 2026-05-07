use aml::prelude::Document;

pub fn render(markup: &str) -> String {
    Document::try_new(markup)
        .expect("invalid aml markup")
        .render()
}
