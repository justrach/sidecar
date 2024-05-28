//! We are going to send a probing request over here
//! to ask for more questions

use crate::agentic::symbol::identifier::SymbolIdentifier;

#[derive(Debug, Clone)]
pub struct SymbolToProbeHistory {
    symbol: String,
    fs_file_path: String,
    content: String,
    question: String,
}

impl SymbolToProbeHistory {
    pub fn new(symbol: String, fs_file_path: String, content: String, question: String) -> Self {
        Self {
            symbol,
            fs_file_path,
            content,
            question,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SymbolToProbeRequest {
    symbol_identifier: SymbolIdentifier,
    probe_request: String,
    history: Vec<SymbolToProbeHistory>,
}

impl SymbolToProbeRequest {
    pub fn new(
        symbol_identifier: SymbolIdentifier,
        probe_request: String,
        history: Vec<SymbolToProbeHistory>,
    ) -> Self {
        Self {
            symbol_identifier,
            probe_request,
            history,
        }
    }

    pub fn probe_request(&self) -> &str {
        &self.probe_request
    }

    pub fn history_slice(&self) -> &[SymbolToProbeHistory] {
        self.history.as_slice()
    }

    pub fn history(&self) -> String {
        self.history
            .iter()
            .map(|history| {
                let symbol = &history.symbol;
                let file_path = &history.fs_file_path;
                let content = &history.content;
                let question = &history.question;
                format!(
                    r#"<item>
<symbol>
{symbol}
</symbol>
<file_path>
{file_path}
</file_path>
<content>
{content}
</content>
<question>
{question}
</question>
</item>"#
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}