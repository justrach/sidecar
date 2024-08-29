//! We are going to log the UI events, this is mostly for
//! debugging and having better visibility to what ever is happening
//! in the symbols

use std::collections::HashMap;

use crate::chunking::text_document::Range;

use super::{
    identifier::SymbolIdentifier,
    types::{SymbolEventRequest, SymbolLocation},
};

#[derive(Debug, serde::Serialize)]
pub struct UIEventWithID {
    request_id: String,
    event: UIEvent,
}

impl UIEventWithID {
    pub fn code_iteration_finished(request_id: String) -> Self {
        Self {
            request_id: request_id.to_owned(),
            event: UIEvent::FrameworkEvent(FrameworkEvent::CodeIterationFinished(request_id)),
        }
    }

    pub fn start_long_context_search(request_id: String) -> Self {
        Self {
            request_id: request_id.to_owned(),
            event: UIEvent::FrameworkEvent(FrameworkEvent::LongContextSearchStart(request_id)),
        }
    }

    pub fn finish_long_context_search(request_id: String) -> Self {
        Self {
            request_id: request_id.to_owned(),
            event: UIEvent::FrameworkEvent(FrameworkEvent::LongContextSearchFinished(request_id)),
        }
    }

    pub fn finish_edit_request(request_id: String) -> Self {
        Self {
            request_id: request_id.to_owned(),
            event: UIEvent::EditRequestFinished(request_id),
        }
    }

    /// Repo map search start
    pub fn repo_map_gen_start(request_id: String) -> Self {
        Self {
            request_id: request_id.to_owned(),
            event: UIEvent::FrameworkEvent(FrameworkEvent::RepoMapGenerationStart(request_id)),
        }
    }

    /// Repo map generation end
    pub fn repo_map_gen_end(request_id: String) -> Self {
        Self {
            request_id: request_id.to_owned(),
            event: UIEvent::FrameworkEvent(FrameworkEvent::RepoMapGenerationFinished(request_id)),
        }
    }

    pub fn from_symbol_event(request_id: String, input: SymbolEventRequest) -> Self {
        Self {
            request_id: request_id,
            event: UIEvent::SymbolEvent(input),
        }
    }

    pub fn symbol_location(request_id: String, symbol_location: SymbolLocation) -> Self {
        Self {
            request_id,
            event: UIEvent::SymbolLoctationUpdate(symbol_location),
        }
    }

    pub fn sub_symbol_step(
        request_id: String,
        sub_symbol_request: SymbolEventSubStepRequest,
    ) -> Self {
        Self {
            request_id,
            event: UIEvent::SymbolEventSubStep(sub_symbol_request),
        }
    }

    pub fn probe_answer_event(
        request_id: String,
        symbol_identifier: SymbolIdentifier,
        probe_answer: String,
    ) -> Self {
        Self {
            request_id,
            event: UIEvent::SymbolEventSubStep(SymbolEventSubStepRequest::new(
                symbol_identifier,
                SymbolEventSubStep::Probe(SymbolEventProbeRequest::ProbeAnswer(probe_answer)),
            )),
        }
    }

    pub fn probing_started_event(request_id: String) -> Self {
        Self {
            request_id,
            event: UIEvent::RequestEvent(RequestEvents::ProbingStart),
        }
    }

    pub fn probing_finished_event(request_id: String, response: String) -> Self {
        Self {
            request_id,
            event: UIEvent::RequestEvent(RequestEvents::ProbeFinished(
                RequestEventProbeFinished::new(response),
            )),
        }
    }

    pub fn range_selection_for_edit(
        request_id: String,
        symbol_identifier: SymbolIdentifier,
        range: Range,
        fs_file_path: String,
    ) -> Self {
        Self {
            request_id,
            event: UIEvent::SymbolEventSubStep(
                SymbolEventSubStepRequest::range_selection_for_edit(
                    symbol_identifier,
                    fs_file_path,
                    range,
                ),
            ),
        }
    }

    pub fn edited_code(
        request_id: String,
        symbol_identifier: SymbolIdentifier,
        range: Range,
        fs_file_path: String,
        edited_code: String,
    ) -> Self {
        Self {
            request_id,
            event: UIEvent::SymbolEventSubStep(SymbolEventSubStepRequest::edited_code(
                symbol_identifier,
                range,
                fs_file_path,
                edited_code,
            )),
        }
    }

    pub fn code_correctness_action(
        request_id: String,
        symbol_identifier: SymbolIdentifier,
        range: Range,
        fs_file_path: String,
        tool_use_thinking: String,
    ) -> Self {
        Self {
            request_id,
            event: UIEvent::SymbolEventSubStep(SymbolEventSubStepRequest::code_correctness_action(
                symbol_identifier,
                range,
                fs_file_path,
                tool_use_thinking,
            )),
        }
    }

    /// Sends the initial search event to the editor
    pub fn initial_search_symbol_event(
        request_id: String,
        symbols: Vec<InitialSearchSymbolInformation>,
    ) -> Self {
        Self {
            request_id: request_id.to_owned(),
            event: UIEvent::FrameworkEvent(FrameworkEvent::InitialSearchSymbols(
                InitialSearchSymbolEvent::new(request_id, symbols),
            )),
        }
    }

    /// sends a open file request
    pub fn open_file_event(request_id: String, fs_file_path: String) -> Self {
        Self {
            request_id: request_id.to_owned(),
            event: UIEvent::FrameworkEvent(FrameworkEvent::OpenFile(OpenFileRequest {
                fs_file_path,
                request_id,
            })),
        }
    }

    // start the edit streaming
    pub fn start_edit_streaming(
        request_id: String,
        symbol_identifier: SymbolIdentifier,
        edit_request_id: String,
        range: Range,
        fs_file_path: String,
    ) -> Self {
        Self {
            request_id,
            event: UIEvent::SymbolEventSubStep(
                SymbolEventSubStepRequest::edited_code_stream_start(
                    symbol_identifier,
                    edit_request_id,
                    range,
                    fs_file_path,
                ),
            ),
        }
    }

    // end the edit streaming
    pub fn end_edit_streaming(
        request_id: String,
        symbol_identifier: SymbolIdentifier,
        edit_request_id: String,
        range: Range,
        fs_file_path: String,
        updated_code: String,
    ) -> Self {
        Self {
            request_id,
            event: UIEvent::SymbolEventSubStep(SymbolEventSubStepRequest::edited_code_stream_end(
                symbol_identifier,
                edit_request_id,
                range,
                fs_file_path,
                updated_code,
            )),
        }
    }

    // send delta from the edit stream
    pub fn delta_edit_streaming(
        request_id: String,
        symbol_identifier: SymbolIdentifier,
        delta: String,
        edit_request_id: String,
        range: Range,
        fs_file_path: String,
    ) -> Self {
        Self {
            request_id,
            event: UIEvent::SymbolEventSubStep(
                SymbolEventSubStepRequest::edited_code_stream_delta(
                    symbol_identifier,
                    edit_request_id,
                    range,
                    fs_file_path,
                    delta,
                ),
            ),
        }
    }

    pub fn send_thinking_for_edit(
        request_id: String,
        symbol_identifier: SymbolIdentifier,
        thinking: String,
        edit_request_id: String,
    ) -> Self {
        Self {
            request_id,
            event: UIEvent::SymbolEventSubStep(SymbolEventSubStepRequest::thinking_for_edit(
                symbol_identifier,
                thinking,
                edit_request_id,
            )),
        }
    }

    pub fn found_reference(request_id: String, references: FoundReference) -> Self {
        Self {
            request_id: request_id.to_owned(),
            event: UIEvent::FrameworkEvent(FrameworkEvent::ReferenceFound(references)),
        }
    }

    pub fn filter_reference_description(request_id: String, answer: &str) -> Self {
        Self {
            request_id: request_id.to_owned(),
            event: UIEvent::FrameworkEvent(FrameworkEvent::ReferenceFilterDescription(
                answer.to_string(),
            )),
        }
    }
}

#[derive(Debug, serde::Serialize)]
pub enum UIEvent {
    SymbolEvent(SymbolEventRequest),
    SymbolLoctationUpdate(SymbolLocation),
    SymbolEventSubStep(SymbolEventSubStepRequest),
    RequestEvent(RequestEvents),
    EditRequestFinished(String),
    FrameworkEvent(FrameworkEvent),
}

impl From<SymbolEventRequest> for UIEvent {
    fn from(req: SymbolEventRequest) -> Self {
        UIEvent::SymbolEvent(req)
    }
}

#[derive(Debug, serde::Serialize)]
pub enum SymbolEventProbeRequest {
    SubSymbolSelection,
    ProbeDeeperSymbol,
    /// The final answer for the probe is sent via this event
    ProbeAnswer(String),
}

#[derive(Debug, serde::Serialize)]
pub struct SymbolEventGoToDefinitionRequest {
    fs_file_path: String,
    range: Range,
    thinking: String,
}

impl SymbolEventGoToDefinitionRequest {
    fn new(fs_file_path: String, range: Range, thinking: String) -> Self {
        Self {
            fs_file_path,
            range,
            thinking,
        }
    }
}

#[derive(Debug, serde::Serialize)]
pub struct RangeSelectionForEditRequest {
    range: Range,
    fs_file_path: String,
}

impl RangeSelectionForEditRequest {
    pub fn new(range: Range, fs_file_path: String) -> Self {
        Self {
            range,
            fs_file_path,
        }
    }
}

#[derive(Debug, serde::Serialize)]
pub struct InsertCodeForEditRequest {
    range: Range,
    fs_file_path: String,
}

#[derive(Debug, serde::Serialize)]
pub struct EditedCodeForEditRequest {
    range: Range,
    fs_file_path: String,
    new_code: String,
}

impl EditedCodeForEditRequest {
    pub fn new(range: Range, fs_file_path: String, new_code: String) -> Self {
        Self {
            range,
            fs_file_path,
            new_code,
        }
    }
}

#[derive(Debug, serde::Serialize)]
pub struct CodeCorrectionToolSelection {
    range: Range,
    fs_file_path: String,
    tool_use_thinking: String,
}

impl CodeCorrectionToolSelection {
    pub fn new(range: Range, fs_file_path: String, tool_use_thinking: String) -> Self {
        Self {
            range,
            fs_file_path,
            tool_use_thinking,
        }
    }
}

#[derive(Debug, serde::Serialize)]
pub enum EditedCodeStreamingEvent {
    Start,
    Delta(String),
    End,
}

#[derive(Debug, serde::Serialize)]
pub struct EditedCodeStreamingRequest {
    edit_request_id: String,
    range: Range,
    fs_file_path: String,
    updated_code: Option<String>,
    event: EditedCodeStreamingEvent,
}

/// We have range selection and then the edited code, we should also show the
/// events which the AI is using for the tool correction and whats it is planning
/// on doing for that
#[derive(Debug, serde::Serialize)]
pub enum SymbolEventEditRequest {
    RangeSelectionForEdit(RangeSelectionForEditRequest),
    /// We might be inserting code at a line which is a new symbol by itself
    InsertCode(InsertCodeForEditRequest),
    EditCode(EditedCodeForEditRequest),
    CodeCorrectionTool(CodeCorrectionToolSelection),
    EditCodeStreaming(EditedCodeStreamingRequest),
    ThinkingForEdit(ThinkingForEditRequest),
}

#[derive(Debug, serde::Serialize)]
pub struct ThinkingForEditRequest {
    edit_request_id: String,
    thinking: String,
}

#[derive(Debug, serde::Serialize)]
pub enum SymbolEventSubStep {
    Probe(SymbolEventProbeRequest),
    GoToDefinition(SymbolEventGoToDefinitionRequest),
    Edit(SymbolEventEditRequest),
}

#[derive(Debug, serde::Serialize)]
pub struct SymbolEventSubStepRequest {
    symbol_identifier: SymbolIdentifier,
    event: SymbolEventSubStep,
}

impl SymbolEventSubStepRequest {
    pub fn new(symbol_identifier: SymbolIdentifier, event: SymbolEventSubStep) -> Self {
        Self {
            symbol_identifier,
            event,
        }
    }

    pub fn probe_answer(symbol_identifier: SymbolIdentifier, answer: String) -> Self {
        Self {
            symbol_identifier,
            event: SymbolEventSubStep::Probe(SymbolEventProbeRequest::ProbeAnswer(answer)),
        }
    }

    pub fn go_to_definition_request(
        symbol_identifier: SymbolIdentifier,
        fs_file_path: String,
        range: Range,
        thinking: String,
    ) -> Self {
        Self {
            symbol_identifier,
            event: SymbolEventSubStep::GoToDefinition(SymbolEventGoToDefinitionRequest::new(
                fs_file_path,
                range,
                thinking,
            )),
        }
    }

    pub fn range_selection_for_edit(
        symbol_identifier: SymbolIdentifier,
        fs_file_path: String,
        range: Range,
    ) -> Self {
        Self {
            symbol_identifier,
            event: SymbolEventSubStep::Edit(SymbolEventEditRequest::RangeSelectionForEdit(
                RangeSelectionForEditRequest::new(range, fs_file_path),
            )),
        }
    }

    pub fn edited_code(
        symbol_identifier: SymbolIdentifier,
        range: Range,
        fs_file_path: String,
        edited_code: String,
    ) -> Self {
        Self {
            symbol_identifier,
            event: SymbolEventSubStep::Edit(SymbolEventEditRequest::EditCode(
                EditedCodeForEditRequest::new(range, fs_file_path, edited_code),
            )),
        }
    }

    pub fn edited_code_stream_start(
        symbol_identifier: SymbolIdentifier,
        edit_request_id: String,
        range: Range,
        fs_file_path: String,
    ) -> Self {
        Self {
            symbol_identifier,
            event: SymbolEventSubStep::Edit(SymbolEventEditRequest::EditCodeStreaming(
                EditedCodeStreamingRequest {
                    edit_request_id,
                    range,
                    fs_file_path,
                    event: EditedCodeStreamingEvent::Start,
                    updated_code: None,
                },
            )),
        }
    }

    pub fn edited_code_stream_end(
        symbol_identifier: SymbolIdentifier,
        edit_request_id: String,
        range: Range,
        fs_file_path: String,
        updated_code: String,
    ) -> Self {
        Self {
            symbol_identifier,
            event: SymbolEventSubStep::Edit(SymbolEventEditRequest::EditCodeStreaming(
                EditedCodeStreamingRequest {
                    edit_request_id,
                    range,
                    fs_file_path,
                    updated_code: Some(updated_code),
                    event: EditedCodeStreamingEvent::End,
                },
            )),
        }
    }

    pub fn thinking_for_edit(
        symbol_identifier: SymbolIdentifier,
        thinking: String,
        edit_request_id: String,
    ) -> Self {
        Self {
            symbol_identifier,
            event: SymbolEventSubStep::Edit(SymbolEventEditRequest::ThinkingForEdit(
                ThinkingForEditRequest {
                    edit_request_id,
                    thinking,
                },
            )),
        }
    }

    pub fn edited_code_stream_delta(
        symbol_identifier: SymbolIdentifier,
        edit_request_id: String,
        range: Range,
        fs_file_path: String,
        delta: String,
    ) -> Self {
        Self {
            symbol_identifier,
            event: SymbolEventSubStep::Edit(SymbolEventEditRequest::EditCodeStreaming(
                EditedCodeStreamingRequest {
                    edit_request_id,
                    range,
                    fs_file_path,
                    event: EditedCodeStreamingEvent::Delta(delta),
                    updated_code: None,
                },
            )),
        }
    }

    pub fn code_correctness_action(
        symbol_identifier: SymbolIdentifier,
        range: Range,
        fs_file_path: String,
        tool_use_thinking: String,
    ) -> Self {
        Self {
            symbol_identifier,
            event: SymbolEventSubStep::Edit(SymbolEventEditRequest::CodeCorrectionTool(
                CodeCorrectionToolSelection::new(range, fs_file_path, tool_use_thinking),
            )),
        }
    }
}

#[derive(Debug, serde::Serialize)]
pub struct RequestEventProbeFinished {
    reply: String,
}

impl RequestEventProbeFinished {
    pub fn new(reply: String) -> Self {
        Self { reply }
    }
}

#[derive(Debug, serde::Serialize)]
pub enum RequestEvents {
    ProbingStart,
    ProbeFinished(RequestEventProbeFinished),
}

#[derive(Debug, serde::Serialize)]
pub struct InitialSearchSymbolInformation {
    symbol_name: String,
    fs_file_path: Option<String>,
    is_new: bool,
    thinking: String,
    // send over the range of this symbol
    range: Option<Range>,
}

impl InitialSearchSymbolInformation {
    pub fn new(
        symbol_name: String,
        fs_file_path: Option<String>,
        is_new: bool,
        thinking: String,
        range: Option<Range>,
    ) -> Self {
        Self {
            symbol_name,
            fs_file_path,
            is_new,
            thinking,
            range,
        }
    }
}

pub type FoundReference = HashMap<String, usize>;

#[derive(Debug, serde::Serialize)]
pub struct InitialSearchSymbolEvent {
    request_id: String,
    symbols: Vec<InitialSearchSymbolInformation>,
}

impl InitialSearchSymbolEvent {
    pub fn new(request_id: String, symbols: Vec<InitialSearchSymbolInformation>) -> Self {
        Self {
            request_id,
            symbols,
        }
    }
}

#[derive(Debug, serde::Serialize)]
pub struct OpenFileRequest {
    fs_file_path: String,
    request_id: String,
}

#[derive(Debug, serde::Serialize)]
pub enum FrameworkEvent {
    RepoMapGenerationStart(String),
    RepoMapGenerationFinished(String),
    LongContextSearchStart(String),
    LongContextSearchFinished(String),
    InitialSearchSymbols(InitialSearchSymbolEvent),
    OpenFile(OpenFileRequest),
    CodeIterationFinished(String),
    ReferenceFound(FoundReference),
    ReferenceFilterDescription(String),
}
