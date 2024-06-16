//! Contains the central manager for the symbols and maintains them in the system
//! as a connected graph in some ways in which these symbols are able to communicate
//! with each other

use std::sync::Arc;

use futures::{stream, StreamExt};
use tokio::sync::mpsc::UnboundedSender;

use crate::agentic::swe_bench::search_cache::LongContextSearchCache;
use crate::agentic::symbol::events::initial_request::InitialRequestData;
use crate::agentic::symbol::events::probe::SymbolToProbeRequest;
use crate::agentic::symbol::events::types::SymbolEvent;
use crate::agentic::symbol::tool_properties::ToolProperties;
use crate::agentic::tool::base::Tool;
use crate::agentic::tool::code_symbol::important::CodeSymbolImportantWideSearch;
use crate::agentic::tool::input::ToolInput;
use crate::chunking::editor_parsing::EditorParsing;
use crate::user_context::types::UserContext;
use crate::{
    agentic::tool::{broker::ToolBroker, output::ToolOutput},
    inline_completion::symbols_tracker::SymbolTrackerInline,
};

use super::identifier::LLMProperties;
use super::tool_box::ToolBox;
use super::ui_event::UIEventWithID;
use super::{
    errors::SymbolError,
    events::input::SymbolInputEvent,
    locker::SymbolLocker,
    types::{SymbolEventRequest, SymbolEventResponse},
};

// This is the main communication manager between all the symbols
// this of this as the central hub through which all the events go forward
pub struct SymbolManager {
    _sender: UnboundedSender<(
        SymbolEventRequest,
        String,
        tokio::sync::oneshot::Sender<SymbolEventResponse>,
    )>,
    // this is the channel where the various symbols will use to talk to the manager
    // which in turn will proxy it to the right symbol, what happens if there are failures
    // each symbol has its own receiver which is being used
    symbol_locker: SymbolLocker,
    tools: Arc<ToolBroker>,
    _symbol_broker: Arc<SymbolTrackerInline>,
    _editor_parsing: Arc<EditorParsing>,
    tool_box: Arc<ToolBox>,
    _editor_url: String,
    _llm_properties: LLMProperties,
    ui_sender: UnboundedSender<UIEventWithID>,
    long_context_cache: LongContextSearchCache,
}

impl SymbolManager {
    pub fn new(
        tools: Arc<ToolBroker>,
        symbol_broker: Arc<SymbolTrackerInline>,
        editor_parsing: Arc<EditorParsing>,
        editor_url: String,
        ui_sender: UnboundedSender<UIEventWithID>,
        llm_properties: LLMProperties,
        // This is a hack and not a proper one at that, we obviously want to
        // do better over here
        user_context: UserContext,
    ) -> Self {
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel::<(
            SymbolEventRequest,
            String,
            tokio::sync::oneshot::Sender<SymbolEventResponse>,
        )>();
        let tool_box = Arc::new(ToolBox::new(
            tools.clone(),
            symbol_broker.clone(),
            editor_parsing.clone(),
            editor_url.to_owned(),
            ui_sender.clone(),
        ));
        let symbol_locker = SymbolLocker::new(
            sender.clone(),
            tool_box.clone(),
            llm_properties.clone(),
            user_context,
            ui_sender.clone(),
        );
        let cloned_symbol_locker = symbol_locker.clone();
        tokio::spawn(async move {
            // TODO(skcd): Make this run in full parallelism in the future, for
            // now this is fine
            while let Some(event) = receiver.recv().await {
                println!("symbol_manager::tokio::spawn::receiver_event");
                // let _ = cloned_ui_sender.send(UIEvent::from(event.0.clone()));
                let _ = cloned_symbol_locker.process_request(event).await;
            }
            println!("symbol_manager::tokio::spawn::end");
        });
        Self {
            _sender: sender,
            symbol_locker,
            _editor_parsing: editor_parsing,
            tools,
            _symbol_broker: symbol_broker,
            tool_box,
            _editor_url: editor_url,
            _llm_properties: llm_properties,
            ui_sender,
            long_context_cache: LongContextSearchCache::new(),
        }
    }

    pub async fn probe_request_from_user_context(
        &self,
        query: String,
        user_context: UserContext,
    ) -> Result<(), SymbolError> {
        println!("symbol_manager::probe_request_from_user_context::start");
        let request_id = uuid::Uuid::new_v4().to_string();
        let request_id_ref = &request_id;
        let variables = self
            .tool_box
            .grab_symbols_from_user_context(
                query.to_owned(),
                user_context.clone(),
                request_id.to_owned(),
            )
            .await;
        let code_wide_search =
            ToolInput::RequestImportantSybmolsCodeWide(CodeSymbolImportantWideSearch::new(
                user_context.clone(),
                query.to_owned(),
                self._llm_properties.llm().clone(),
                self._llm_properties.provider().clone(),
                self._llm_properties.api_key().clone(),
            ));
        // send the request on to the ui sender
        let _ = self.ui_sender.send(UIEventWithID::from_tool_event(
            request_id_ref.to_owned(),
            code_wide_search.clone(),
        ));
        let output = {
            match variables {
                Ok(variables) => ToolOutput::ImportantSymbols(variables),
                _ => self
                    .tools
                    .invoke(code_wide_search)
                    .await
                    .map_err(|e| SymbolError::ToolError(e))?,
            }
        };
        println!(
            "symbol_manager::probe_request_from_user_context::output({:?})",
            &output
        );
        if let ToolOutput::ImportantSymbols(important_symbols)
        | ToolOutput::RepoMapSearch(important_symbols) = output
        {
            // We have the important symbols here which we can then use to invoke the individual process request
            let important_symbols = important_symbols.fix_symbol_names();

            let mut symbols = self
                .tool_box
                .important_symbols(important_symbols.clone(), user_context.clone(), &request_id)
                .await
                .map_err(|e| e.into())?;
            // TODO(skcd): Another check over here is that we can search for the exact variable
            // and then ask for the edit
            println!("Symbols over here: {:?}", &symbols);
            // TODO(skcd): the symbol here might belong to a class or it might be a global function
            // we want to grab the largest node containing the symbol here instead of using
            // the symbol directly since our algorithm would not work otherwise
            // we would also need to de-duplicate the symbols which we have to process right over here
            // otherwise it might lead to errors
            if symbols.is_empty() {
                println!("symbol_manager::grab_symbols_using_search");
                symbols = self
                    .tool_box
                    .grab_symbol_using_search(important_symbols, user_context.clone(), &request_id)
                    .await
                    .map_err(|e| e.into())?;
            }

            // Create all the symbol agents
            let symbol_identifiers = stream::iter(symbols)
                .map(|symbol_request| async move {
                    let symbol_identifier = self
                        .symbol_locker
                        .create_symbol_agent(
                            symbol_request,
                            request_id_ref.to_owned(),
                            ToolProperties::new(),
                        )
                        .await;
                    symbol_identifier
                })
                .buffer_unordered(100)
                .collect::<Vec<_>>()
                .await
                .into_iter()
                .filter_map(|s| s.ok())
                .collect::<Vec<_>>();

            println!(
                "symbol_manager::probe_request_from_user_context::len({})",
                symbol_identifiers.len(),
            );

            let query_ref = &query;

            // Now for all the symbol identifiers which we are getting we have to
            // send a request to all of them with the probe query
            let responses = stream::iter(symbol_identifiers.into_iter().map(|symbol_identifier| {
                (
                    symbol_identifier.clone(),
                    request_id_ref.to_owned(),
                    SymbolEventRequest::probe_request(
                        symbol_identifier.clone(),
                        SymbolToProbeRequest::new(
                            symbol_identifier,
                            query_ref.to_owned(),
                            query_ref.to_owned(),
                            request_id_ref.to_owned(),
                            vec![],
                        ),
                        ToolProperties::new(),
                    ),
                )
            }))
            .map(
                |(symbol_identifier, request_id, symbol_event_request)| async move {
                    let (sender, receiver) = tokio::sync::oneshot::channel();
                    dbg!(
                        "sending initial request to symbol: {:?}",
                        &symbol_identifier
                    );
                    self.symbol_locker
                        .process_request((symbol_event_request, request_id, sender))
                        .await;
                    let response = receiver.await;
                    dbg!(
                        "For symbol identifier: {:?} the response is {:?}",
                        &symbol_identifier,
                        &response
                    );
                    (response, symbol_identifier)
                },
            )
            .buffer_unordered(100)
            .collect::<Vec<_>>()
            .await;

            // send the response forward after combining all the answers using the LLM
            let final_responses = responses
                .into_iter()
                .filter_map(|(response, symbol_identifier)| match response {
                    Ok(response) => Some((response, symbol_identifier)),
                    _ => None,
                })
                .collect::<Vec<_>>();
            let final_answer = final_responses
                .into_iter()
                .map(|(response, symbol_identifier)| {
                    let symbol_name = symbol_identifier.symbol_name();
                    let symbol_file_path = symbol_identifier
                        .fs_file_path()
                        .map(|fs_file_path| format!("at {}", fs_file_path))
                        .unwrap_or("".to_owned());
                    let response = response.to_string();
                    let symbol_readable = format!("{symbol_name} {symbol_file_path}");
                    format!(
                        r#"{symbol_readable}
{response}"#
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            let _ = self.ui_sender.send(UIEventWithID::probing_finished_event(
                request_id_ref.to_owned(),
                final_answer,
            ));
            println!("things are completed over here after probing");
        }
        println!("things are more complete over here after probing");
        Ok(())
    }

    // This is just for testing out the flow for single input events
    pub async fn probe_request(&self, input_event: SymbolEventRequest) -> Result<(), SymbolError> {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let request_id = uuid::Uuid::new_v4().to_string();
        let _ = self
            .symbol_locker
            .process_request((input_event, request_id, sender))
            .await;
        let response = receiver.await;
        println!("{:?}", response.expect("to work"));
        Ok(())
    }

    // once we have the initial request, which we will go through the initial request
    // mode once, we have the symbols from it we can use them to spin up sub-symbols as well
    pub async fn initial_request(&self, input_event: SymbolInputEvent) -> Result<(), SymbolError> {
        let user_context = input_event.provided_context().clone();
        let request_id = uuid::Uuid::new_v4().to_string();
        let _ = self.ui_sender.send(UIEventWithID::for_codebase_event(
            request_id.to_owned(),
            input_event.clone(),
        ));
        let swe_bench_id = input_event.swe_bench_instance_id();
        let swe_bench_git_dname = input_event.get_swe_bench_git_dname();
        let swe_bench_test_endpoint = input_event.get_swe_bench_test_endpoint();
        let swe_bench_code_editing_model = input_event.get_swe_bench_code_editing();
        let swe_bench_gemini_properties = input_event.get_swe_bench_gemini_llm_properties();
        let tool_properties = ToolProperties::new()
            .set_swe_bench_endpoint(swe_bench_test_endpoint.clone())
            .set_swe_bench_code_editing_llm(swe_bench_code_editing_model)
            .set_swe_bench_reranking_llm(swe_bench_gemini_properties);
        let tool_properties_ref = &tool_properties;
        let user_query = input_event.user_query().to_owned();
        let tool_input = input_event.tool_use_on_initial_invocation().await;
        println!("Tool input: {:?}", &tool_input);
        if let Some(tool_input) = tool_input {
            // send the tool input to the ui sender
            // we need some kind of cache here so we do not invoke the expensive
            // query so many times
            let _ = self.ui_sender.send(UIEventWithID::from_tool_event(
                request_id.to_owned(),
                tool_input.clone(),
            ));
            let important_symbols = if let Some(swe_bench_id) = swe_bench_id.to_owned() {
                let symbols = self.long_context_cache.check_cache(&swe_bench_id).await;
                if let Some(_git_dname) = swe_bench_git_dname {
                    match symbols {
                        Some(symbols) => Some(symbols),
                        None => None,
                    }
                } else {
                    symbols
                }
            } else {
                None
            };

            println!("Important symbols {:?}", &important_symbols);
            let tool_output = match important_symbols {
                Some(important_symbols) => ToolOutput::RepoMapSearch(important_symbols),
                None => self
                    .tools
                    .invoke(tool_input)
                    .await
                    .map_err(|e| SymbolError::ToolError(e))?,
            };

            if let ToolOutput::ImportantSymbols(important_symbols)
            | ToolOutput::RepoMapSearch(important_symbols) = tool_output
            {
                // The fix symbol name here helps us get the top-level symbol name
                // if the LLM decides to have fun and spit out a.b.c instead of a or b or c individually
                // as it can with python where it will tell class.method_name instead of just class or just
                // method_name
                let important_symbols = important_symbols.fix_symbol_names();
                // swe bench caching hit over here we just do it
                self.long_context_cache
                    .update_cache(swe_bench_id, &important_symbols)
                    .await;

                // Debug printing
                println!("Important symbols: {:?}", &important_symbols);

                let mut symbols = self
                    .tool_box
                    .important_symbols(important_symbols.clone(), user_context.clone(), &request_id)
                    .await
                    .map_err(|e| e.into())?;
                // TODO(skcd): Another check over here is that we can search for the exact variable
                // and then ask for the edit
                println!("Symbols over here: {:?}", &symbols);
                // TODO(skcd): the symbol here might belong to a class or it might be a global function
                // we want to grab the largest node containing the symbol here instead of using
                // the symbol directly since our algorithm would not work otherwise
                // we would also need to de-duplicate the symbols which we have to process right over here
                // otherwise it might lead to errors
                if symbols.is_empty() {
                    println!("symbol_manager::grab_symbols_using_search");
                    symbols = self
                        .tool_box
                        .grab_symbol_using_search(
                            important_symbols,
                            user_context.clone(),
                            &request_id,
                        )
                        .await
                        .map_err(|e| e.into())?;
                }

                let request_id_ref = &request_id;
                // This is where we are creating all the symbols
                let symbol_identifiers = stream::iter(symbols)
                    .map(|symbol_request| async move {
                        let symbol_identifier = self
                            .symbol_locker
                            .create_symbol_agent(
                                symbol_request,
                                request_id_ref.to_owned(),
                                tool_properties_ref.clone(),
                            )
                            .await;
                        symbol_identifier
                    })
                    .buffer_unordered(100)
                    .collect::<Vec<_>>()
                    .await
                    .into_iter()
                    .filter_map(|s| s.ok())
                    .collect::<Vec<_>>();

                dbg!("Symbol identifiers size: ({})", symbol_identifiers.len());
                // Once we have the symbols spinning up, we send them the original request
                // which the user had and send it over and then we can await on all of them
                // working at the same time.
                dbg!("initial request");
                let _ = stream::iter(symbol_identifiers.into_iter().map(|symbol_identifier| {
                    (
                        symbol_identifier.clone(),
                        request_id_ref.to_owned(),
                        SymbolEventRequest::new(
                            symbol_identifier,
                            SymbolEvent::InitialRequest(InitialRequestData::new(
                                user_query.to_owned(),
                            )),
                            tool_properties_ref.clone(),
                        ),
                    )
                }))
                .map(
                    |(symbol_identifier, request_id, symbol_event_request)| async move {
                        let (sender, receiver) = tokio::sync::oneshot::channel();
                        dbg!(
                            "sending initial request to symbol: {:?}",
                            &symbol_identifier
                        );
                        self.symbol_locker
                            .process_request((symbol_event_request, request_id, sender))
                            .await;
                        let response = receiver.await;
                        dbg!(
                            "For symbol identifier: {:?} the response is {:?}",
                            &symbol_identifier,
                            &response
                        );
                    },
                )
                .buffer_unordered(100)
                .collect::<Vec<_>>()
                .await;
            }
        } else {
            // We are for some reason not even invoking the first passage which is
            // weird, this is completely wrong and we should be replying back
            println!("this is wrong, please look at the comment here");
        }
        Ok(())
    }
}
