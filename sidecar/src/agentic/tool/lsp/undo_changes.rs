//! Undo the changes we made during a session
//! We have access to a session and exchange and the plan step

use async_trait::async_trait;

use crate::agentic::tool::{errors::ToolError, input::ToolInput, output::ToolOutput, r#type::Tool};

#[derive(Debug, Clone, serde::Serialize)]
pub struct UndoChangesMadeDuringExchangeRequest {
    exchange_id: String,
    session_id: String,
    // this is the plan step index if we are going to undo until then
    index: Option<usize>,
    editor_url: String,
}

impl UndoChangesMadeDuringExchangeRequest {
    pub fn new(
        exchange_id: String,
        session_id: String,
        index: Option<usize>,
        editor_url: String,
    ) -> Self {
        Self {
            exchange_id,
            session_id,
            index,
            editor_url,
        }
    }
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct UndoChangesMadeDuringExchangeRespnose {
    success: bool,
}

impl UndoChangesMadeDuringExchangeRespnose {
    pub fn is_success(&self) -> bool {
        self.success
    }
}

pub struct UndoChangesMadeDuringExchange {
    client: reqwest::Client,
}

impl UndoChangesMadeDuringExchange {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl Tool for UndoChangesMadeDuringExchange {
    async fn invoke(&self, input: ToolInput) -> Result<ToolOutput, ToolError> {
        let context = input.is_undo_request_during_session()?;
        let endpoint_url = context.editor_url.to_owned() + "/undo_session_changes";
        let response = self
            .client
            .post(endpoint_url)
            .body(serde_json::to_string(&context).map_err(|_e| ToolError::SerdeConversionFailed)?)
            .send()
            .await
            .map_err(|_e| ToolError::ErrorCommunicatingWithEditor)?;
        let response: UndoChangesMadeDuringExchangeRespnose = response
            .json()
            .await
            .map_err(|_e| ToolError::SerdeConversionFailed)?;
        Ok(ToolOutput::undo_changes_made_during_session(response))
    }
}