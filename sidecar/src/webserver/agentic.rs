//! Contains the handler for agnetic requests and how they work

use super::model_selection::LLMClientConfig;
use super::types::json as json_result;
use axum::response::{sse, IntoResponse, Sse};
use axum::{extract::Query as axumQuery, Extension, Json};
use futures::{stream, StreamExt};
use llm_client::{
    clients::types::LLMType,
    provider::{AnthropicAPIKey, LLMProvider, LLMProviderAPIKeys},
};
use serde_json::json;
use std::collections::HashMap;
use std::time::Instant;
use std::{sync::Arc, time::Duration};
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

use super::types::Result;
use crate::agentic::symbol::anchored::AnchoredSymbol;
use crate::agentic::symbol::events::environment_event::EnvironmentEventType;
use crate::agentic::symbol::events::human::{HumanAgenticRequest, HumanMessage};
use crate::agentic::symbol::events::input::SymbolEventRequestId;
use crate::agentic::symbol::events::lsp::{LSPDiagnosticError, LSPSignal};
use crate::agentic::symbol::events::message_event::SymbolEventMessageProperties;
use crate::agentic::symbol::helpers::SymbolFollowupBFS;
use crate::agentic::symbol::scratch_pad::ScratchPadAgent;
use crate::agentic::symbol::tool_properties::ToolProperties;
use crate::agentic::symbol::toolbox::helpers::SymbolChangeSet;
use crate::agentic::symbol::ui_event::{RelevantReference, UIEventWithID};
use crate::agentic::tool::input::ToolInput;
use crate::agentic::tool::lsp::open_file::OpenFileResponse;
use crate::agentic::tool::r#type::Tool;
use crate::agentic::tool::ref_filter::ref_filter::{ReferenceFilterBroker, ReferenceFilterRequest};
use crate::chunking::text_document::Range;
use crate::{
    agentic::symbol::identifier::LLMProperties, application::application::Application,
    user_context::types::UserContext,
};

use super::types::ApiResponse;

/// Tracks and manages probe requests in a concurrent environment.
/// This struct is responsible for keeping track of ongoing probe requests
pub struct ProbeRequestTracker {
    /// A thread-safe map of running requests, keyed by request ID.
    ///
    /// - Key: String representing the unique request ID.
    /// - Value: JoinHandle for the asynchronous task handling the request.
    pub running_requests: Arc<Mutex<HashMap<String, JoinHandle<()>>>>,
}

impl ProbeRequestTracker {
    pub fn new() -> Self {
        Self {
            running_requests: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    async fn track_new_request(&self, request_id: &str, join_handle: JoinHandle<()>) {
        let mut running_requests = self.running_requests.lock().await;
        running_requests.insert(request_id.to_owned(), join_handle);
    }

    async fn cancel_request(&self, request_id: &str) {
        let mut running_requests = self.running_requests.lock().await;
        if let Some(response) = running_requests.get_mut(request_id) {
            // we abort the running requests
            response.abort();
        }
    }
}

/// Contains all the data which we will need to trigger the edits
/// Represents metadata for anchored editing operations.
#[derive(Clone)]
struct AnchoredEditingMetadata {
    /// Properties of the message event associated with this editing session.
    message_properties: SymbolEventMessageProperties,
    /// The symbols that are currently focused on in the selection.
    /// These are the primary targets for the editing operation.
    anchored_symbols: Vec<AnchoredSymbol>,
    /// Stores the original content of the files mentioned before editing started.
    /// This allows for comparison and potential rollback if needed.
    /// Key: File path, Value: Original file content
    previous_file_content: HashMap<String, String>,
    /// Stores references to the anchor selection nodes.
    /// These references can be used for navigation or additional context during editing.
    references: Vec<RelevantReference>,
    /// Optional string representing the user's context for this editing session.
    /// This can provide additional information or constraints for the editing process.
    user_context_string: Option<String>,
    /// environment events
    environment_event_sender: UnboundedSender<EnvironmentEventType>,
    /// the scratchpad agent which tracks the state of the request
    _scratch_pad_agent: ScratchPadAgent,
}

impl AnchoredEditingMetadata {
    pub fn new(
        message_properties: SymbolEventMessageProperties,
        anchored_symbols: Vec<AnchoredSymbol>,
        previous_file_content: HashMap<String, String>,
        references: Vec<RelevantReference>,
        user_context_string: Option<String>,
        scratch_pad_agent: ScratchPadAgent,
        environment_event_sender: UnboundedSender<EnvironmentEventType>,
    ) -> Self {
        Self {
            message_properties,
            anchored_symbols,
            previous_file_content,
            references,
            user_context_string,
            _scratch_pad_agent: scratch_pad_agent,
            environment_event_sender,
        }
    }

    pub fn references(&self) -> &[RelevantReference] {
        &self.references
    }

    pub fn anchored_symbols(&self) -> &[AnchoredSymbol] {
        &self.anchored_symbols
    }
}

pub struct AnchoredEditingTracker {
    // right now our cache is made up of file path to the file content and this is the cache
    // which we pass to the agents when we startup
    // we update the cache only when we have a hit on a new request
    cache_right_now: Arc<Mutex<Vec<OpenFileResponse>>>,
    running_requests_properties: Arc<Mutex<HashMap<String, AnchoredEditingMetadata>>>,
    running_requests: Arc<Mutex<HashMap<String, JoinHandle<()>>>>,
}

impl AnchoredEditingTracker {
    pub fn new() -> Self {
        Self {
            cache_right_now: Arc::new(Mutex::new(vec![])),
            running_requests_properties: Arc::new(Mutex::new(HashMap::new())),
            running_requests: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    async fn get_properties(&self, request_id: &str) -> Option<AnchoredEditingMetadata> {
        let running_requests = self.running_requests_properties.lock().await;
        running_requests.get(request_id).map(|data| data.clone())
    }

    /// this replaces the existing references field
    async fn add_reference(&self, request_id: &str, relevant_refs: &[RelevantReference]) {
        let mut running_request_properties = self.running_requests_properties.lock().await;

        if let Some(metadata) = running_request_properties.get_mut(request_id) {
            metadata.references = relevant_refs.to_vec();
        } else {
            println!("No metadata found for request_id: {}", request_id);
        }
    }

    // consider better error handling
    pub async fn add_join_handle(
        &self,
        request_id: &str,
        join_handle: JoinHandle<()>,
    ) -> Result<(), String> {
        let mut running_requests = self.running_requests.lock().await;
        if running_requests.contains_key(request_id) {
            running_requests.insert(request_id.to_owned(), join_handle);
            Ok(())
        } else {
            Err(format!(
                "No existing request found for request_id: {}",
                request_id
            ))
        }
    }

    async fn track_new_request(
        &self,
        request_id: &str,
        join_handle: Option<JoinHandle<()>>, // Optional to allow asynchronous composition of requests
        editing_metadata: Option<AnchoredEditingMetadata>, // Optional to allow asynchronous composition of requests
    ) {
        {
            let mut running_requests = self.running_requests.lock().await;
            if let Some(join_handle) = join_handle {
                running_requests.insert(request_id.to_owned(), join_handle);
            }
        }
        {
            let mut running_request_properties = self.running_requests_properties.lock().await;
            if let Some(metadata) = editing_metadata {
                running_request_properties.insert(request_id.to_owned(), metadata);
            }
        }
    }

    pub async fn override_running_request(&self, request_id: &str, join_handle: JoinHandle<()>) {
        {
            let mut running_requests = self.running_requests.lock().await;
            running_requests.insert(request_id.to_owned(), join_handle);
        }
    }

    pub async fn send_diagnostics_event(&self, diagnostics: Vec<LSPDiagnosticError>) {
        let environment_senders;
        {
            let running_request_properties = self.running_requests_properties.lock().await;
            environment_senders = running_request_properties
                .iter()
                .map(|running_properties| running_properties.1.environment_event_sender.clone())
                .collect::<Vec<_>>();
        }
        environment_senders
            .into_iter()
            .for_each(|environment_sender| {
                let _ = environment_sender.send(EnvironmentEventType::LSP(LSPSignal::diagnostics(
                    diagnostics.to_vec(),
                )));
            })
    }

    // Update the cache which we are sending over to the agent
    pub async fn update_cache(&self) {}
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ProbeRequestActiveWindow {
    file_path: String,
    file_content: String,
    language: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ProbeRequest {
    request_id: String,
    editor_url: String,
    model_config: LLMClientConfig,
    user_context: UserContext,
    query: String,
    active_window_data: Option<ProbeRequestActiveWindow>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ProbeStopRequest {
    request_id: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ProbeStopResponse {
    done: bool,
}

pub async fn probe_request_stop(
    Extension(app): Extension<Application>,
    Json(ProbeStopRequest { request_id }): Json<ProbeStopRequest>,
) -> Result<impl IntoResponse> {
    println!("webserver::probe_request_stop");
    let probe_request_tracker = app.probe_request_tracker.clone();
    let _ = probe_request_tracker.cancel_request(&request_id).await;
    Ok(Json(ProbeStopResponse { done: true }))
}

pub async fn probe_request(
    Extension(app): Extension<Application>,
    Json(ProbeRequest {
        request_id,
        editor_url,
        model_config,
        mut user_context,
        query,
        active_window_data,
    }): Json<ProbeRequest>,
) -> Result<impl IntoResponse> {
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    let probe_request_tracker = app.probe_request_tracker.clone();
    if let Some(active_window_data) = active_window_data {
        user_context = user_context.update_file_content_map(
            active_window_data.file_path,
            active_window_data.file_content,
            active_window_data.language,
        );
    }
    let provider_keys = model_config
        .provider_for_slow_model()
        .map(|provider| provider.clone())
        .ok_or(anyhow::anyhow!("missing provider for slow model"))?;
    let _provider_type = provider_keys.provider_type();
    let event_message_properties = SymbolEventMessageProperties::new(
        SymbolEventRequestId::new(request_id.to_owned(), request_id.to_owned()),
        sender.clone(),
        editor_url,
    );

    let symbol_manager = app.symbol_manager.clone();

    // spawn a background thread to keep polling the probe_request future
    let join_handle = tokio::spawn(async move {
        let _ = symbol_manager
            .probe_request_from_user_context(query, user_context, event_message_properties.clone())
            .await;
    });

    let _ = probe_request_tracker
        .track_new_request(&request_id, join_handle)
        .await;

    // Now we want to poll the future of the probe request we are sending
    // along with the ui events so we can return the channel properly
    // how do go about doing that?
    let event_stream = Sse::new(
        tokio_stream::wrappers::UnboundedReceiverStream::new(receiver).map(|event| {
            sse::Event::default()
                .json_data(event)
                .map_err(anyhow::Error::new)
        }),
    );

    // return the stream as a SSE event stream over here
    Ok(event_stream.keep_alive(
        sse::KeepAlive::new()
            .interval(Duration::from_secs(3))
            .event(
                sse::Event::default()
                    .json_data(json!({
                        "keep_alive": "alive"
                    }))
                    .expect("json to not fail in keep alive"),
            ),
    ))
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct SWEBenchRequest {
    git_dname: String,
    problem_statement: String,
    editor_url: String,
    test_endpoint: String,
    // This is the file path with the repo map present in it
    repo_map_file: Option<String>,
    gcloud_access_token: String,
    swe_bench_id: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SweBenchCompletionResponse {
    done: bool,
}

impl ApiResponse for SweBenchCompletionResponse {}

pub async fn swe_bench(
    axumQuery(SWEBenchRequest {
        git_dname: _git_dname,
        problem_statement: _problem_statement,
        editor_url: _editor_url,
        test_endpoint: _test_endpoint,
        repo_map_file: _repo_map_file,
        gcloud_access_token: _glcoud_access_token,
        swe_bench_id: _swe_bench_id,
    }): axumQuery<SWEBenchRequest>,
    Extension(_app): Extension<Application>,
) -> Result<impl IntoResponse> {
    // let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    // let tool_broker = Arc::new(ToolBroker::new(
    //     app.llm_broker.clone(),
    //     Arc::new(CodeEditBroker::new()),
    //     app.symbol_tracker.clone(),
    //     app.language_parsing.clone(),
    //     // for swe-bench tests we do not care about tracking edits
    //     ToolBrokerConfiguration::new(None, true),
    //     LLMProperties::new(
    //         LLMType::GeminiPro,
    //         LLMProvider::GoogleAIStudio,
    //         LLMProviderAPIKeys::GoogleAIStudio(GoogleAIStudioKey::new(
    //             "AIzaSyCMkKfNkmjF8rTOWMg53NiYmz0Zv6xbfsE".to_owned(),
    //         )),
    //     ),
    // ));
    // let user_context = UserContext::new(vec![], vec![], None, vec![git_dname]);
    // let model = LLMType::ClaudeSonnet;
    // let provider_type = LLMProvider::Anthropic;
    // let anthropic_api_keys = LLMProviderAPIKeys::Anthropic(AnthropicAPIKey::new("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".to_owned()));
    // let symbol_manager = SymbolManager::new(
    //     tool_broker,
    //     app.symbol_tracker.clone(),
    //     app.editor_parsing.clone(),
    //     LLMProperties::new(
    //         model.clone(),
    //         provider_type.clone(),
    //         anthropic_api_keys.clone(),
    //     ),
    // );

    // let message_properties = SymbolEventMessageProperties::new(
    //     SymbolEventRequestId::new(swe_bench_id.to_owned(), swe_bench_id.to_owned()),
    //     sender.clone(),
    //     editor_url.to_owned(),
    // );

    println!("we are getting a hit at this endpoint");

    // Now we send the original request over here and then await on the sender like
    // before
    // tokio::spawn(async move {
    //     let _ = symbol_manager
    //         .initial_request(
    //             SymbolInputEvent::new(
    //                 user_context,
    //                 model,
    //                 provider_type,
    //                 anthropic_api_keys,
    //                 problem_statement,
    //                 "web_server_input".to_owned(),
    //                 "web_server_input".to_owned(),
    //                 Some(test_endpoint),
    //                 repo_map_file,
    //                 None,
    //                 None,
    //                 None,
    //                 None,
    //                 None,
    //                 false,
    //                 None,
    //                 None,
    //                 false,
    //                 sender,
    //             )
    //             .set_swe_bench_id(swe_bench_id),
    //             message_properties,
    //         )
    //         .await;
    // });
    // let event_stream = Sse::new(
    //     tokio_stream::wrappers::UnboundedReceiverStream::new(receiver).map(|event| {
    //         sse::Event::default()
    //             .json_data(event)
    //             .map_err(anyhow::Error::new)
    //     }),
    // );

    // // return the stream as a SSE event stream over here
    // Ok(event_stream.keep_alive(
    //     sse::KeepAlive::new()
    //         .interval(Duration::from_secs(3))
    //         .event(
    //             sse::Event::default()
    //                 .json_data(json!({
    //                     "keep_alive": "alive"
    //                 }))
    //                 .expect("json to not fail in keep alive"),
    //         ),
    // ))
    Ok(json_result(SweBenchCompletionResponse { done: true }))
}

/// Represents a request to warm up the code sculpting system.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CodeSculptingWarmup {
    file_paths: Vec<String>,
    grab_import_nodes: bool,
    editor_url: String,
}

/// Response structure for the code sculpting warmup operation.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CodeSculptingWarmupResponse {
    done: bool,
}

impl ApiResponse for CodeSculptingWarmupResponse {}

pub async fn code_sculpting_warmup(
    Extension(app): Extension<Application>,
    Json(CodeSculptingWarmup {
        file_paths,
        grab_import_nodes,
        editor_url,
    }): Json<CodeSculptingWarmup>,
) -> Result<impl IntoResponse> {
    println!("webserver::code_sculpting_warmup");
    println!(
        "webserver::code_sculpting_warmup::file_paths({})",
        file_paths.to_vec().join(",")
    );
    let warmup_request_id = "warmup_request_id".to_owned();
    let (sender, _receiver) = tokio::sync::mpsc::unbounded_channel();
    let message_properties = SymbolEventMessageProperties::new(
        SymbolEventRequestId::new(warmup_request_id.to_owned(), warmup_request_id.to_owned()),
        sender,
        editor_url,
    );
    let files_already_in_cache;
    {
        files_already_in_cache = app
            .anchored_request_tracker
            .cache_right_now
            .lock()
            .await
            .iter()
            .map(|open_file_response| open_file_response.fs_file_path().to_owned())
            .collect::<Vec<_>>();
    }
    // if the order of files which we are tracking is the same and there is no difference
    // then we should not update our cache
    if files_already_in_cache == file_paths {
        return Ok(json_result(CodeSculptingWarmupResponse { done: true }));
    }
    let mut file_cache_vec = vec![];
    for file_path in file_paths.into_iter() {
        let file_content = app
            .tool_box
            .file_open(file_path, message_properties.clone())
            .await;
        if let Ok(file_content) = file_content {
            file_cache_vec.push(file_content);
        }
    }

    // Now we put this in our cache over here
    {
        let mut file_caches = app.anchored_request_tracker.cache_right_now.lock().await;
        *file_caches = file_cache_vec.to_vec();
    }
    let _ = app
        .tool_box
        .warmup_context(file_cache_vec, grab_import_nodes, message_properties)
        .await;
    Ok(json_result(CodeSculptingWarmupResponse { done: true }))
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CodeSculptingHeal {
    request_id: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CodeSculptingHealResponse {
    done: bool,
}

impl ApiResponse for CodeSculptingHealResponse {}

pub async fn code_sculpting_heal(
    Extension(app): Extension<Application>,
    Json(CodeSculptingHeal { request_id }): Json<CodeSculptingHeal>,
) -> Result<impl IntoResponse> {
    println!(
        "webserver::code_sculpting_heal::request_id({})",
        &request_id
    );
    let anchor_properties;
    {
        let anchor_tracker = app.anchored_request_tracker.clone();
        anchor_properties = anchor_tracker.get_properties(&request_id).await;
    }
    println!(
        "code_sculpting::heal::request_id({})::properties_present({})",
        request_id,
        anchor_properties.is_some()
    );
    if anchor_properties.is_none() {
        Ok(json_result(CodeSculptingHealResponse { done: false }))
    } else {
        let anchor_properties = anchor_properties.expect("is_none to hold");

        let anchored_symbols = anchor_properties.anchored_symbols();

        let relevant_references = anchor_properties.references();
        println!(
            "agentic::webserver::code_sculpting_heal::relevant_references.len({})",
            relevant_references.len()
        );

        let file_paths = anchored_symbols
            .iter()
            .filter_map(|r| r.fs_file_path())
            .collect::<Vec<_>>();

        let older_file_content_map = anchor_properties.previous_file_content;
        let message_properties = anchor_properties.message_properties.clone();

        // Now grab the symbols which have changed
        let cloned_tools = app.tool_box.clone();
        let symbol_change_set: HashMap<String, SymbolChangeSet> =
            stream::iter(file_paths.into_iter().map(|file_path| {
                let older_file_content = older_file_content_map
                    .get(&file_path)
                    .map(|content| content.to_owned());
                (
                    file_path,
                    cloned_tools.clone(),
                    older_file_content,
                    message_properties.clone(),
                )
            }))
            .map(
                |(fs_file_path, tools, older_file_content, message_properties)| async move {
                    if let Some(older_content) = older_file_content {
                        let file_content = tools
                            .file_open(fs_file_path.to_owned(), message_properties)
                            .await
                            .ok();
                        if let Some(new_content) = file_content {
                            tools
                                .get_symbol_change_set(
                                    &fs_file_path,
                                    &older_content,
                                    new_content.contents_ref(),
                                )
                                .await
                                .ok()
                                .map(|symbol_change_set| (fs_file_path, symbol_change_set))
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                },
            )
            .buffer_unordered(10)
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .filter_map(|s| s)
            .collect::<HashMap<_, _>>();

        let changed_symbols = anchor_properties
            .anchored_symbols
            .into_iter()
            .filter_map(|anchored_symbol| {
                let symbol_identifier = anchored_symbol.identifier().to_owned();
                let fs_file_path = symbol_identifier.fs_file_path();
                if fs_file_path.is_none() {
                    return None;
                }
                let fs_file_path = fs_file_path.clone().expect("is_none to hold");
                let changed_symbols_in_file = symbol_change_set.get(&fs_file_path);
                if let Some(changed_symbols_in_file) = changed_symbols_in_file {
                    let symbol_changes = changed_symbols_in_file
                        .changes()
                        .into_iter()
                        .filter(|changed_symbol| {
                            changed_symbol.symbol_identifier().symbol_name()
                                == symbol_identifier.symbol_name()
                        })
                        .map(|changed_symbol| changed_symbol.clone())
                        .collect::<Vec<_>>();
                    Some(symbol_changes)
                } else {
                    None
                }
            })
            .flatten()
            .collect::<Vec<_>>();

        println!(
            "webserver::agentic::changed_symbols: \n{:?}",
            &changed_symbols
        );

        // changed symbols also has symbol_identifier
        let followup_bfs_request = changed_symbols
            .into_iter()
            .map(|changes| {
                let symbol_identifier = changes.symbol_identifier().clone();
                let symbol_identifier_ref = &symbol_identifier;
                changes
                    .remove_changes()
                    .into_iter()
                    .map(|symbol_to_edit| {
                        SymbolFollowupBFS::new(
                            symbol_to_edit.0,
                            symbol_identifier_ref.clone(),
                            symbol_to_edit.1,
                            symbol_to_edit.2,
                        )
                    })
                    .collect::<Vec<_>>()
            })
            .flatten()
            .collect::<Vec<_>>();
        // make sure that the edit request we are creating is on the whole outline
        // node and not on the individual function

        let hub_sender = app.symbol_manager.hub_sender();
        let cloned_tools = app.tool_box.clone();
        let _join_handle = tokio::spawn(async move {
            let _ = cloned_tools
                .check_for_followups_bfs(
                    followup_bfs_request,
                    hub_sender,
                    message_properties.clone(),
                    &ToolProperties::new(),
                )
                .await;

            // send event after we are done with the followups
            let ui_sender = message_properties.ui_sender();
            let _ = ui_sender.send(UIEventWithID::finish_edit_request(
                message_properties.request_id_str().to_owned(),
            ));
        });
        Ok(json_result(CodeSculptingHealResponse { done: true }))
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CodeSculptingRequest {
    request_id: String,
    instruction: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CodeSculptingResponse {
    done: bool,
}

impl ApiResponse for CodeSculptingResponse {}

pub async fn code_sculpting(
    Extension(app): Extension<Application>,
    Json(CodeSculptingRequest {
        request_id,
        instruction,
    }): Json<CodeSculptingRequest>,
) -> Result<impl IntoResponse> {
    let anchor_properties;
    {
        let anchor_tracker = app.anchored_request_tracker.clone();
        anchor_properties = anchor_tracker.get_properties(&request_id).await;
    }
    println!(
        "code_sculpting::instruction({})::properties_present({})",
        instruction,
        anchor_properties.is_some()
    );
    if anchor_properties.is_none() {
        Ok(json_result(CodeSculptingResponse { done: false }))
    } else {
        let anchor_properties = anchor_properties.expect("is_none to hold");
        let join_handle = tokio::spawn(async move {
            let anchored_symbols = anchor_properties.anchored_symbols;
            let user_provided_context = anchor_properties.user_context_string;
            let environment_sender = anchor_properties.environment_event_sender;
            let _ = environment_sender.send(EnvironmentEventType::human_anchor_request(
                instruction,
                anchored_symbols,
                user_provided_context,
            ));
        });
        {
            let anchor_tracker = app.anchored_request_tracker.clone();
            let _ = anchor_tracker
                .override_running_request(&request_id, join_handle)
                .await;
        }
        Ok(json_result(CodeSculptingResponse { done: true }))
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AgenticCodeEditing {
    user_query: String,
    editor_url: String,
    request_id: String,
    user_context: UserContext,
    active_window_data: Option<ProbeRequestActiveWindow>,
    root_directory: String,
    codebase_search: bool,
    // If we are editing based on an anchor position
    anchor_editing: bool,
    enable_import_nodes: bool,
}

pub async fn code_editing(
    Extension(app): Extension<Application>,
    Json(AgenticCodeEditing {
        user_query,
        editor_url,
        request_id,
        mut user_context,
        active_window_data,
        root_directory,
        codebase_search,
        anchor_editing,
        enable_import_nodes,
    }): Json<AgenticCodeEditing>,
) -> Result<impl IntoResponse> {
    println!("webserver::code_editing_start::request_id({})", &request_id);
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    if let Some(active_window_data) = active_window_data {
        user_context = user_context.update_file_content_map(
            active_window_data.file_path,
            active_window_data.file_content,
            active_window_data.language,
        );
    }

    let message_properties = SymbolEventMessageProperties::new(
        SymbolEventRequestId::new(request_id.to_owned(), request_id.to_owned()),
        sender.clone(),
        editor_url,
    );

    println!(
        "webserver::code_editing_flow::endpoint_hit::anchor_editing({})",
        anchor_editing
    );
    if anchor_editing {
        println!(
            "webserver::code_editing_flow::anchor_editing::({})",
            anchor_editing
        );
        let symbols_to_anchor = app
            .tool_box
            .symbols_to_anchor(&user_context, message_properties.clone())
            .await
            .unwrap_or_default();
        println!(
            "webserver::code_editing_flow::anchor_symbols::({})",
            symbols_to_anchor
                .iter()
                .map(|anchored_symbol| anchored_symbol.name())
                .collect::<Vec<_>>()
                .join(",")
        );
        let metadata_pregen = Instant::now();

        let mut user_provided_context = app
            .tool_box
            .file_paths_to_user_context(
                user_context.file_paths(),
                enable_import_nodes,
                message_properties.clone(),
            )
            .await
            .ok();

        // get the git diff of the repository at this state right now
        // this is also passed as user context which goes into warmup and stays
        // as context throughout the run of this request_id
        // this will lead to broken output when we retrigger the anchor again
        // we have to maintain this cache in a better way
        // the storage has to be like:
        // - L2: files_provided
        // - L1: git_diff (can change on each invocation but not really, we should have
        // a better delta detection)
        // - register: active changes which we have made
        let git_diff = app.tool_box.get_git_diff(&root_directory).await;
        if let Ok(git_diff) = git_diff {
            let git_diff = git_diff.new_version();
            user_provided_context = user_provided_context.map(|user_context| {
                format!(
                    r#"{user_context}
<diff_of_changes>
{git_diff}
</diff_of_changes>"#
                )
            });
        }
        let possibly_changed_files = symbols_to_anchor
            .iter()
            .filter_map(|anchored_symbol| anchored_symbol.fs_file_path())
            .collect::<Vec<_>>();
        let cloned_tools = app.tool_box.clone();

        // consider whether this is necessary
        let file_contents = stream::iter(
            possibly_changed_files
                .into_iter()
                .map(|file_path| (file_path, cloned_tools.clone(), message_properties.clone())),
        )
        .map(|(fs_file_path, tools, message_properties)| async move {
            let file_open_response = tools
                .file_open(fs_file_path.to_owned(), message_properties)
                .await;
            file_open_response
                .ok()
                .map(|response| (fs_file_path, response.contents()))
        })
        .buffer_unordered(100)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .filter_map(|s| s)
        .collect::<HashMap<_, _>>();

        println!("metadata_pregen::elapsed({:?})", metadata_pregen.elapsed());

        // the storage unit for the scratch pad path
        // create this file path before we start editing it
        let mut scratch_pad_file_path = app.config.scratch_pad().join(request_id.to_owned());
        scratch_pad_file_path.set_extension("md");
        let (scratch_pad_agent, environment_sender) = ScratchPadAgent::start_scratch_pad(
            scratch_pad_file_path,
            app.tool_box.clone(),
            app.symbol_manager.hub_sender(),
            message_properties.clone(),
            user_provided_context.clone(),
        )
        .await;

        let editing_metadata = AnchoredEditingMetadata::new(
            message_properties.clone(),
            symbols_to_anchor.clone(),
            file_contents,
            vec![],
            user_provided_context.clone(),
            scratch_pad_agent,
            // we store the environment sender so we can use it later for
            // sending the scratchpad some events
            environment_sender.clone(),
        );

        println!("tracking new request");
        // instantiates basic request tracker, with no join_handle, but basic metadata
        let _ = app
            .anchored_request_tracker
            .track_new_request(&request_id, None, Some(editing_metadata))
            .await;

        println!("tracked new request");

        // shit this should be cleaned up
        let cloned_symbols_to_anchor = symbols_to_anchor.clone();
        let cloned_message_properties = message_properties.clone();
        let cloned_toolbox = app.tool_box.clone();
        let cloned_request_id = request_id.clone();
        let cloned_tracker = app.anchored_request_tracker.clone();
        let cloned_user_query = user_query.clone();

        if !symbols_to_anchor.is_empty() {
            let stream_symbols = cloned_symbols_to_anchor.clone();
            let _references_join_handle = tokio::spawn(async move {
                let start = Instant::now();

                let references = stream::iter(stream_symbols.into_iter())
                    .flat_map(|anchored_symbol| {
                        let symbol_names = anchored_symbol.sub_symbol_names().to_vec();
                        let symbol_identifier = anchored_symbol.identifier().to_owned();
                        let toolbox = cloned_toolbox.clone();
                        let message_properties = cloned_message_properties.clone();
                        let request_id = cloned_request_id.clone();
                        let range = anchored_symbol.possible_range().clone();
                        stream::iter(symbol_names.into_iter().filter_map(move |symbol_name| {
                            symbol_identifier.fs_file_path().map(|path| {
                                (
                                    anchored_symbol.clone(),
                                    path,
                                    symbol_name,
                                    toolbox.clone(),
                                    message_properties.clone(),
                                    request_id.clone(),
                                    range.clone(),
                                )
                            })
                        }))
                    })
                    .map(
                        |(
                            original_symbol,
                            path,
                            symbol_name,
                            toolbox,
                            message_properties,
                            request_id,
                            range,
                        )| async move {
                            println!("getting references for {}-{}", &path, &symbol_name);
                            let refs = toolbox
                                .get_symbol_references(
                                    path,
                                    symbol_name.to_owned(),
                                    range,
                                    message_properties.clone(),
                                    request_id.clone(),
                                )
                                .await;

                            match refs {
                                Ok(references) => {
                                    toolbox
                                        .anchored_references_for_locations(
                                            references.as_slice(),
                                            original_symbol,
                                            message_properties,
                                        )
                                        .await
                                }
                                Err(e) => {
                                    println!("{:?}", e);
                                    vec![]
                                }
                            }
                        },
                    )
                    .buffer_unordered(100)
                    .collect::<Vec<_>>()
                    .await
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>();

                println!("total references: {}", references.len());
                println!("collect references time elapsed: {:?}", start.elapsed());

                // send UI event with grouped references
                let grouped: HashMap<String, usize> = references.clone().into_iter().fold(
                    HashMap::new(),
                    |mut acc, anchored_reference| {
                        let reference_len = anchored_reference.reference_locations().len();
                        acc.entry(
                            anchored_reference
                                .fs_file_path_for_outline_node()
                                .to_string(),
                        )
                        .and_modify(|count| *count += reference_len)
                        .or_insert(1);
                        acc
                    },
                );

                let _ = cloned_message_properties.clone().ui_sender().send(
                    UIEventWithID::found_reference(cloned_request_id.clone(), grouped),
                );

                let llm_broker = app.llm_broker;

                let anthropic_api_keys = LLMProviderAPIKeys::Anthropic(AnthropicAPIKey::new("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".to_owned()));
                let llm_properties = LLMProperties::new(
                    LLMType::ClaudeSonnet,
                    LLMProvider::Anthropic,
                    anthropic_api_keys.clone(),
                );

                let reference_filter_broker =
                    ReferenceFilterBroker::new(llm_broker, llm_properties.clone());

                let references = references
                    .into_iter()
                    .take(10) // todo(zi): so we don't go crazy with 1000s of requests
                    .collect::<Vec<_>>();

                println!(
                    "code_editing:reference_symbols.len({:?})",
                    &references.len()
                );

                let request = ReferenceFilterRequest::new(
                    cloned_user_query,
                    llm_properties.clone(),
                    cloned_request_id.clone(),
                    cloned_message_properties.clone(),
                    references.clone(),
                );

                let llm_time = Instant::now();
                println!("ReferenceFilter::invoke::start");

                let relevant_references = match reference_filter_broker
                    .invoke(ToolInput::ReferencesFilter(request))
                    .await
                {
                    Ok(ok_references) => {
                        ok_references.get_relevant_references().unwrap_or_default()
                    }
                    Err(err) => {
                        eprintln!("Failed to filter references: {:?}", err);
                        Vec::new()
                    }
                };

                let _ = cloned_tracker
                    .add_reference(&cloned_request_id, &relevant_references)
                    .await;

                println!("ReferenceFilter::invoke::elapsed({:?})", llm_time.elapsed());

                println!(
                    "collect references async task total elapsed: {:?}",
                    start.elapsed()
                );
                relevant_references
            });
            // end of async task

            let cloned_user_context = user_provided_context.clone();
            // no way to monitor the speed of response over here, which sucks but
            // we can figure that out later
            let cloned_environment_sender = environment_sender.clone();

            let join_handle = tokio::spawn(async move {
                let _ = cloned_environment_sender.send(EnvironmentEventType::human_anchor_request(
                    user_query,
                    symbols_to_anchor,
                    cloned_user_context,
                ));
            });

            let _ = app
                .anchored_request_tracker
                .add_join_handle(&request_id, join_handle)
                .await;
            let properties_present = app
                .anchored_request_tracker
                .get_properties(&request_id)
                .await;

            println!(
                "webserver::anchored_edits::request_id({})::properties_present({})",
                &request_id,
                properties_present.is_some()
            );

            // there will never be references at this point, given this runs well before the join_handles can resolve
            println!(
                "webserver::anchored_edits::request_id({})::properties_present({}).references.len({})",
                &request_id,
                properties_present.is_some(),
                properties_present.map_or(0, |p| p.references().len())
            );
        }
    } else {
        println!("webserver::code_editing_flow::agentic_editing");

        // recreate the scratch-pad agent here, give us a hot minute before we figure out
        // how to merge these 2 flows together
        let (environment_sender, environment_receiver) = tokio::sync::mpsc::unbounded_channel();

        // the storage unit for the scratch pad path
        // create this file path before we start editing it
        let mut scratch_pad_file_path = app.config.scratch_pad().join(request_id.to_owned());
        scratch_pad_file_path.set_extension("md");
        let mut scratch_pad_file = tokio::fs::File::create(scratch_pad_file_path.clone())
            .await
            .expect("scratch_pad path created");
        let _ = scratch_pad_file
            .write_all("<scratchpad>\n</scratchpad>".as_bytes())
            .await;
        let _ = scratch_pad_file
            .flush()
            .await
            .expect("initiating scratch pad failed");
        let user_provided_context = app
            .tool_box
            .file_paths_to_user_context(
                user_context.file_paths(),
                enable_import_nodes,
                message_properties.clone(),
            )
            .await
            .ok();

        let scratch_pad_path = scratch_pad_file_path
            .into_os_string()
            .into_string()
            .expect("os_string to into_string to work");
        let scratch_pad_agent = ScratchPadAgent::new(
            scratch_pad_path,
            message_properties.clone(),
            app.tool_box.clone(),
            app.symbol_manager.hub_sender(),
            user_provided_context.clone(),
        )
        .await;
        let _scratch_pad_handle = tokio::spawn(async move {
            // spawning the scratch pad agent
            scratch_pad_agent
                .process_envrionment(Box::pin(
                    tokio_stream::wrappers::UnboundedReceiverStream::new(environment_receiver),
                ))
                .await;
        });
        let _ = environment_sender.send(EnvironmentEventType::Human(HumanMessage::Agentic(
            HumanAgenticRequest::new(user_query, root_directory, codebase_search, user_context),
        )));
    }

    let event_stream = Sse::new(
        tokio_stream::wrappers::UnboundedReceiverStream::new(receiver).map(|event| {
            sse::Event::default()
                .json_data(event)
                .map_err(anyhow::Error::new)
        }),
    );

    // return the stream as a SSE event stream over here
    Ok(event_stream.keep_alive(
        sse::KeepAlive::new()
            .interval(Duration::from_secs(3))
            .event(
                sse::Event::default()
                    .json_data(json!({
                        "keep_alive": "alive"
                    }))
                    .expect("json to not fail in keep alive"),
            ),
    ))
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AgenticDiagnosticData {
    message: String,
    range: Range,
    range_content: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AgenticDiagnostics {
    fs_file_path: String,
    diagnostics: Vec<AgenticDiagnosticData>,
    source: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AgenticDiagnosticsResponse {
    done: bool,
}

impl ApiResponse for AgenticDiagnosticsResponse {}

pub async fn push_diagnostics(
    Extension(app): Extension<Application>,
    Json(AgenticDiagnostics {
        fs_file_path,
        diagnostics,
        source: _source,
    }): Json<AgenticDiagnostics>,
) -> Result<impl IntoResponse> {
    // implement this api endpoint properly and send events over to the right
    // scratch-pad agent
    let lsp_diagnostics = diagnostics
        .into_iter()
        .map(|webserver_diagnostic| {
            LSPDiagnosticError::new(
                webserver_diagnostic.range,
                webserver_diagnostic.range_content,
                fs_file_path.to_owned(),
                webserver_diagnostic.message,
            )
        })
        .collect::<Vec<_>>();

    // now look at all the active scratch-pad agents and send them this event
    let _ = app
        .anchored_request_tracker
        .send_diagnostics_event(lsp_diagnostics)
        .await;
    Ok(json_result(AgenticDiagnosticsResponse { done: true }))
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AnchorSessionStart {
    request_id: String,
    user_context: UserContext,
    editor_url: String,
    active_window_data: Option<ProbeRequestActiveWindow>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AnchorSessionStartResponse {
    done: bool,
}

impl ApiResponse for AnchorSessionStartResponse {}

pub async fn anchor_session_start(
    Extension(app): Extension<Application>,
    Json(AnchorSessionStart {
        request_id,
        mut user_context,
        editor_url,
        active_window_data,
    }): Json<AnchorSessionStart>,
) -> Result<impl IntoResponse> {
    println!(
        "webserver::anchor_session_start::request_id({})",
        &request_id
    );

    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    if let Some(active_window_data) = active_window_data {
        user_context = user_context.update_file_content_map(
            active_window_data.file_path,
            active_window_data.file_content,
            active_window_data.language,
        );
    }

    let message_properties = SymbolEventMessageProperties::new(
        SymbolEventRequestId::new(request_id.to_owned(), request_id.to_owned()),
        sender.clone(),
        editor_url,
    );

    println!(
        "webserver::agentic::anchor_session_start::user_context::variables:\n{}",
        user_context
            .variables
            .iter()
            .map(|var| var.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );

    let _symbol_to_anchor = app
        .tool_box
        .symbols_to_anchor(&user_context, message_properties.clone())
        .await
        .unwrap_or_default();

    let event_stream = Sse::new(
        tokio_stream::wrappers::UnboundedReceiverStream::new(receiver).map(|event| {
            sse::Event::default()
                .json_data(event)
                .map_err(anyhow::Error::new)
        }),
    );

    Ok(event_stream.keep_alive(
        sse::KeepAlive::new()
            .interval(Duration::from_secs(3))
            .event(
                sse::Event::default()
                    .json_data(json!({
                        "keep_alive": "alive"
                    }))
                    .expect("json to not fail in keep alive"),
            ),
    ))
}
