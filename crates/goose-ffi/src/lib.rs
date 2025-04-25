use std::ffi::{c_char, CStr, CString};
use std::ptr;
use std::sync::Arc;

use futures::StreamExt;
use goose::agents::Agent;
use goose::message::Message;
use goose::model::ModelConfig;
use goose::providers::databricks::DatabricksProvider;
use goose_llm::{completion, CompletionResponse};
use once_cell::sync::OnceCell;
use tokio::runtime::Runtime;
use goose::providers::errors::ProviderError;
use mcp_core::tool::{Tool, ToolAnnotations};
use anyhow::Result;
use serde_json::Value;

// This class is in alpha and not yet ready for production use
// and the API is not yet stable. Use at your own risk.

// Thread-safe global runtime
static RUNTIME: OnceCell<Runtime> = OnceCell::new();

// Get or initialize the global runtime
fn get_runtime() -> &'static Runtime {
    RUNTIME.get_or_init(|| {
        // Runtime with all features enabled
        Runtime::new().expect("Failed to create Tokio runtime")
    })
}

/// Pointer type for the agent
pub type AgentPtr = *mut Agent;
/// Provider Type enumeration
/// Currently only Databricks is supported
#[repr(u32)]
#[derive(Debug, Clone, Copy)]
pub enum ProviderType {
    /// Databricks AI provider
    Databricks = 0,
}

/// Provider configuration used to initialize an AI provider
///
/// - provider_type: Provider type (0 = Databricks, other values will produce an error)
/// - api_key: Provider API key (null for default from environment variables)
/// - model_name: Model name to use (null for provider default)
/// - host: Provider host URL (null for default from environment variables)
#[repr(C)]
pub struct ProviderConfigFFI {
    pub provider_type: ProviderType,
    pub api_key: *const c_char,
    pub model_name: *const c_char,
    pub host: *const c_char,
}

// Extension configuration will be implemented in a future commit

/// Role enum for message participants
#[repr(u32)]
#[derive(Debug, Clone, Copy)]
pub enum MessageRole {
    /// User message role
    User = 0,
    /// Assistant message role
    Assistant = 1,
    /// System message role
    System = 2,
}

/// Tool definition for use with completion
///
/// - name: Tool name
/// - description: Tool description
/// - input_schema_json: JSON schema for the tool's input parameters
#[repr(C)]
pub struct ToolFFI {
    pub name: *const c_char,
    pub description: *const c_char,
    pub input_schema_json: *const c_char,
}

/// Message structure for agent interactions
///
/// - role: Message role (User, Assistant, or System)
/// - content: Text content of the message
#[repr(C)]
pub struct MessageFFI {
    pub role: MessageRole,
    pub content: *const c_char,
}

// Tool callbacks will be implemented in a future commit

/// Result type for async operations
///
/// - succeeded: true if the operation succeeded, false otherwise
/// - error_message: Error message if succeeded is false, NULL otherwise
#[repr(C)]
pub struct AsyncResult {
    pub succeeded: bool,
    pub error_message: *mut c_char,
}

/// Free an async result structure
///
/// This function frees the memory allocated for an AsyncResult structure,
/// including any error message it contains.
///
/// # Safety
///
/// The result pointer must be a valid pointer returned by a goose FFI function,
/// or NULL.
#[no_mangle]
pub unsafe extern "C" fn goose_free_async_result(result: *mut AsyncResult) {
    if !result.is_null() {
        let result = &mut *result;
        if !result.error_message.is_null() {
            let _ = CString::from_raw(result.error_message);
        }
        let _ = Box::from_raw(result);
    }
}

/// Create a new agent with the given provider configuration
///
/// # Parameters
///
/// - config: Provider configuration
///
/// # Returns
///
/// A new agent pointer, or a null pointer if creation failed
///
/// # Safety
///
/// The config pointer must be valid or NULL. The resulting agent must be freed
/// with goose_agent_free when no longer needed.
#[no_mangle]
pub unsafe extern "C" fn goose_agent_new(config: *const ProviderConfigFFI) -> AgentPtr {
    // Check for null pointer
    if config.is_null() {
        eprintln!("Error: config pointer is null");
        return ptr::null_mut();
    }

    let config = &*config;

    // We currently only support Databricks provider
    // This match ensures future compiler errors if new provider types are added without handling
    match config.provider_type {
        ProviderType::Databricks => (), // Databricks provider is supported
    }

    // Get api_key from config or environment
    let api_key = if !config.api_key.is_null() {
        CStr::from_ptr(config.api_key).to_string_lossy().to_string()
    } else {
        match std::env::var("DATABRICKS_API_KEY") {
            Ok(key) => key,
            Err(_) => {
                eprintln!("Error: api_key not provided and DATABRICKS_API_KEY environment variable not set");
                return ptr::null_mut();
            }
        }
    };

    // Check and get required model_name (no env fallback for model)
    if config.model_name.is_null() {
        eprintln!("Error: model_name is required but was null");
        return ptr::null_mut();
    }
    let model_name = CStr::from_ptr(config.model_name)
        .to_string_lossy()
        .to_string();

    // Get host from config or environment
    let host = if !config.host.is_null() {
        CStr::from_ptr(config.host).to_string_lossy().to_string()
    } else {
        match std::env::var("DATABRICKS_HOST") {
            Ok(url) => url,
            Err(_) => {
                eprintln!(
                    "Error: host not provided and DATABRICKS_HOST environment variable not set"
                );
                return ptr::null_mut();
            }
        }
    };

    // Create model config with model name
    let model_config = ModelConfig::new(model_name);

    // Create Databricks provider with required parameters
    match DatabricksProvider::from_params(host, api_key, model_config) {
        Ok(provider) => {
            let agent = Agent::new();
            get_runtime().block_on(async {
                let _ = agent.update_provider(Arc::new(provider)).await;
            });
            Box::into_raw(Box::new(agent))
        }
        Err(e) => {
            eprintln!("Error creating Databricks provider: {:?}", e);
            ptr::null_mut()
        }
    }
}

/// Free an agent
///
/// This function frees the memory allocated for an agent.
///
/// # Parameters
///
/// - agent_ptr: Agent pointer returned by goose_agent_new
///
/// # Safety
///
/// The agent_ptr must be a valid pointer returned by goose_agent_new,
/// or have a null internal pointer. The agent_ptr must not be used after
/// calling this function.
#[no_mangle]
pub unsafe extern "C" fn goose_agent_free(agent_ptr: AgentPtr) {
    if !agent_ptr.is_null() {
        let _ = Box::from_raw(agent_ptr);
    }
}

/// Send a message to the agent and get the response
///
/// This function sends a message to the agent and returns the response.
/// Tool handling is not yet supported and will be implemented in a future commit
/// so this may change significantly
///
/// # Parameters
///
/// - agent_ptr: Agent pointer
/// - message: Message to send
///
/// # Returns
///
/// A C string with the agent's response, or NULL on error.
/// This string must be freed with goose_free_string when no longer needed.
///
/// # Safety
///
/// The agent_ptr must be a valid pointer returned by goose_agent_new.
/// The message must be a valid C string.
#[no_mangle]
pub unsafe extern "C" fn goose_agent_send_message(
    agent_ptr: AgentPtr,
    message: *const c_char,
) -> *mut c_char {
    if agent_ptr.is_null() || message.is_null() {
        return ptr::null_mut();
    }

    let agent = &mut *agent_ptr;
    let message = CStr::from_ptr(message).to_string_lossy().to_string();

    let messages = vec![Message::user().with_text(&message)];

    // Block on the async call using our global runtime
    let response = get_runtime().block_on(async {
        let mut stream = match agent.reply(&messages, None).await {
            Ok(stream) => stream,
            Err(e) => return format!("Error getting reply from agent: {}", e),
        };

        let mut full_response = String::new();

        while let Some(message_result) = stream.next().await {
            match message_result {
                Ok(message) => {
                    // Get text or serialize to JSON
                    // Note: Message doesn't have as_text method, we'll serialize to JSON
                    if let Ok(json) = serde_json::to_string(&message) {
                        full_response.push_str(&json);
                    }
                }
                Err(e) => {
                    full_response.push_str(&format!("\nError in message stream: {}", e));
                }
            }
        }
        full_response
    });

    string_to_c_char(&response)
}

// Tool schema creation will be implemented in a future commit

/// Free a string allocated by goose FFI functions
///
/// This function frees memory allocated for strings returned by goose FFI functions.
///
/// # Parameters
///
/// - s: String to free
///
/// # Safety
///
/// The string must have been allocated by a goose FFI function, or be NULL.
/// The string must not be used after calling this function.
#[no_mangle]
pub unsafe extern "C" fn goose_free_string(s: *mut c_char) {
    if !s.is_null() {
        let _ = CString::from_raw(s);
    }
}

// Helper function to convert a Rust string to a C char pointer
fn string_to_c_char(s: &str) -> *mut c_char {
    match CString::new(s) {
        Ok(c_string) => c_string.into_raw(),
        Err(_) => ptr::null_mut(),
    }
}

/// Completion response structure
///
/// - content: JSON string containing the completion response
/// - succeeded: true if the operation succeeded, false otherwise
/// - error_message: Error message if succeeded is false, NULL otherwise
#[repr(C)]
pub struct CompletionResponseFFI {
    pub content: *mut c_char,
    pub succeeded: bool,
    pub error_message: *mut c_char,
}

/// Free a completion response structure
///
/// This function frees the memory allocated for a CompletionResponseFFI structure,
/// including any content and error message it contains.
///
/// # Safety
///
/// The response pointer must be a valid pointer returned by a goose FFI function,
/// or NULL.
#[no_mangle]
pub unsafe extern "C" fn goose_free_completion_response(response: *mut CompletionResponseFFI) {
    println!("goose_free_completion_response: Freeing completion response");
    if !response.is_null() {
        let response = &mut *response;
        if !response.content.is_null() {
            println!("goose_free_completion_response: Freeing content");
            let _ = CString::from_raw(response.content);
        }
        if !response.error_message.is_null() {
            println!("goose_free_completion_response: Freeing error message");
            let _ = CString::from_raw(response.error_message);
        }
        println!("goose_free_completion_response: Freeing response struct");
        let _ = Box::from_raw(response);
    } else {
        println!("goose_free_completion_response: Response was null, nothing to free");
    }
    println!("goose_free_completion_response: Done");
}

/// Perform a completion request
///
/// This function sends a completion request to the specified provider and returns
/// the response.
///
/// # Parameters
///
/// - provider: Provider name (e.g., "databricks", "anthropic")
/// - model_name: Model name to use
/// - host: Provider host URL (NULL for default from environment variables)
/// - api_key: Provider API key (NULL for default from environment variables)
/// - system_preamble: System preamble text
/// - messages: Array of MessageFFI structures
/// - message_count: Number of messages in the array
/// - tools: Array of ToolFFI structures
/// - tool_count: Number of tools in the array
/// - check_tool_approval: Whether to check tool approvals
///
/// # Returns
///
/// A CompletionResponseFFI structure containing the response or error.
/// This must be freed with goose_free_completion_response when no longer needed.
///
/// # Safety
///
/// All string parameters must be valid C strings or NULL.
/// The messages array must contain valid MessageFFI structures.
/// The tools array must contain valid ToolFFI structures.
#[no_mangle]
pub unsafe extern "C" fn goose_completion(
    provider: *const c_char,
    model_name: *const c_char,
    host: *const c_char,
    api_key: *const c_char,
    system_preamble: *const c_char,
    messages_ptr: *const MessageFFI,
    message_count: usize,
    tools_ptr: *const ToolFFI,
    tool_count: usize,
    check_tool_approval: bool,
) -> *mut CompletionResponseFFI {
    println!("goose_completion: Starting completion request");
    
    // Check for null pointers
    if provider.is_null() || model_name.is_null() || system_preamble.is_null() || (messages_ptr.is_null() && message_count > 0) {
        let error_msg = "Error: One or more required parameters are null";
        println!("goose_completion: {}", error_msg);
        return create_error_response(error_msg);
    }

    // Convert C strings to Rust strings
    let provider_str = match CStr::from_ptr(provider).to_str() {
        Ok(s) => s,
        Err(_) => {
            println!("goose_completion: Invalid provider string");
            return create_error_response("Error: Invalid provider string");
        }
    };
    println!("goose_completion: Using provider: {}", provider_str);

    let model_name_str = match CStr::from_ptr(model_name).to_str() {
        Ok(s) => s,
        Err(_) => {
            println!("goose_completion: Invalid model name string");
            return create_error_response("Error: Invalid model name string");
        }
    };
    println!("goose_completion: Using model: {}", model_name_str);

    let system_preamble_str = match CStr::from_ptr(system_preamble).to_str() {
        Ok(s) => s,
        Err(_) => {
            println!("goose_completion: Invalid system preamble string");
            return create_error_response("Error: Invalid system preamble string");
        }
    };
    println!("goose_completion: System preamble length: {} characters", system_preamble_str.len());

    // Convert optional host and api_key parameters
    let host_str = if !host.is_null() {
        match CStr::from_ptr(host).to_str() {
            Ok(s) => {
                println!("goose_completion: Using provided host: {}", s);
                Some(s.to_string())
            },
            Err(_) => {
                println!("goose_completion: Invalid host string");
                return create_error_response("Error: Invalid host string");
            }
        }
    } else {
        println!("goose_completion: No host provided, will use environment variable if available");
        None
    };

    let api_key_str = if !api_key.is_null() {
        match CStr::from_ptr(api_key).to_str() {
            Ok(s) => {
                println!("goose_completion: Using provided API key (redacted)");
                Some(s.to_string())
            },
            Err(_) => {
                println!("goose_completion: Invalid API key string");
                return create_error_response("Error: Invalid api_key string");
            }
        }
    } else {
        println!("goose_completion: No API key provided, will use environment variable if available");
        None
    };

    // Convert FFI messages to Rust messages
    println!("goose_completion: Converting {} messages", message_count);
    let mut rust_messages = Vec::with_capacity(message_count);
    for i in 0..message_count {
        let ffi_message = &*messages_ptr.add(i);
        
        // Check for null content
        if ffi_message.content.is_null() {
            println!("goose_completion: Message {} has null content", i);
            return create_error_response("Error: Message content is null");
        }
        
        let content = match CStr::from_ptr(ffi_message.content).to_str() {
            Ok(s) => s,
            Err(_) => {
                println!("goose_completion: Message {} has invalid content string", i);
                return create_error_response("Error: Invalid message content string");
            }
        };
        
        let role = match ffi_message.role {
            MessageRole::User => "user",
            MessageRole::Assistant => "assistant",
            MessageRole::System => "system",
        };
        println!("goose_completion: Message {}: role={}, content length={}", i, role, content.len());
        
        let message = match ffi_message.role {
            MessageRole::User => Message::user().with_text(content),
            MessageRole::Assistant => Message::assistant().with_text(content),
            MessageRole::System => {
                // For system messages, we'll use a user message with a special prefix
                // since there's no system role in the actual Role enum
                Message::user().with_text(format!("[SYSTEM]: {}", content))
            },
        };
        
        rust_messages.push(message);
    }

    // Convert FFI tools to Rust tools
    println!("goose_completion: Converting {} tools", tool_count);
    let mut rust_tools = Vec::with_capacity(tool_count);
    for i in 0..tool_count {
        let ffi_tool = &*tools_ptr.add(i);
        
        // Check for null pointers
        if ffi_tool.name.is_null() || ffi_tool.description.is_null() || ffi_tool.input_schema_json.is_null() {
            println!("goose_completion: Tool {} has null fields", i);
            return create_error_response("Error: Tool has null name, description, or input schema");
        }
        
        // Convert name and description
        let name = match CStr::from_ptr(ffi_tool.name).to_str() {
            Ok(s) => s.to_string(),
            Err(_) => {
                println!("goose_completion: Tool {} has invalid name string", i);
                return create_error_response("Error: Invalid tool name string");
            }
        };
        
        let description = match CStr::from_ptr(ffi_tool.description).to_str() {
            Ok(s) => s.to_string(),
            Err(_) => {
                println!("goose_completion: Tool {} has invalid description string", i);
                return create_error_response("Error: Invalid tool description string");
            }
        };
        
        // Parse input schema JSON
        let input_schema_str = match CStr::from_ptr(ffi_tool.input_schema_json).to_str() {
            Ok(s) => s,
            Err(_) => {
                println!("goose_completion: Tool {} has invalid input schema string", i);
                return create_error_response("Error: Invalid tool input schema string");
            }
        };
        
        let input_schema: Value = match serde_json::from_str(input_schema_str) {
            Ok(v) => v,
            Err(e) => {
                println!("goose_completion: Tool {} has invalid JSON in input schema: {}", i, e);
                return create_error_response(&format!("Error parsing tool input schema JSON: {}", e));
            }
        };
        
        // Use default annotations - read-only and idempotent
        let annotations = Some(ToolAnnotations {
            title: None,
            read_only_hint: true,  // Default to read-only for safety
            destructive_hint: false,
            idempotent_hint: true,
            open_world_hint: true,
        });
        
        println!("goose_completion: Tool {}: name={}, description length={}", i, name, description.len());
        
        // Create the Tool
        let tool = Tool::new(name, description, input_schema, annotations);
        rust_tools.push(tool);
    }

    // Create model config
    println!("goose_completion: Creating model config for {}", model_name_str);
    let model_config = ModelConfig::new(model_name_str.to_string());
    
    // Perform the completion using our global runtime
    println!("goose_completion: Starting async completion request");
    let result: Result<CompletionResponse, ProviderError> = get_runtime().block_on(async {
        // Set environment variables if host and api_key are provided
        if let Some(host_value) = &host_str {
            let env_var_name = format!("{}_HOST", provider_str.to_uppercase());
            println!("goose_completion: Setting environment variable: {}", env_var_name);
            std::env::set_var(&env_var_name, host_value);
        }
        
        if let Some(api_key_value) = &api_key_str {
            let env_var_name = format!("{}_API_KEY", provider_str.to_uppercase());
            println!("goose_completion: Setting environment variable: {}", env_var_name);
            std::env::set_var(&env_var_name, api_key_value);
        }

        println!("goose_completion: Calling completion function with {} tools", rust_tools.len());
        let result = completion(
            provider_str,
            model_config,
            system_preamble_str,
            &rust_messages,
            &rust_tools,
            check_tool_approval,
        ).await;
        
        println!("goose_completion: Completion function returned: {}", 
            if result.is_ok() { "success" } else { "error" });
        
        result
    });

    match result {
        Ok(response) => {
            // Serialize the response to JSON
            println!("goose_completion: Serializing successful response to JSON");
            match serde_json::to_string(&response) {
                Ok(json) => {
                    println!("goose_completion: JSON serialization successful, length={}", json.len());
                    let content = string_to_c_char(&json);
                    let response = Box::new(CompletionResponseFFI {
                        content,
                        succeeded: true,
                        error_message: ptr::null_mut(),
                    });
                    println!("goose_completion: Returning successful response");
                    Box::into_raw(response)
                }
                Err(e) => {
                    let error_msg = format!("Error serializing response: {}", e);
                    println!("goose_completion: {}", error_msg);
                    create_error_response(&error_msg)
                }
            }
        }
        Err(e) => {
            let error_msg = format!("Completion error: {}", e);
            println!("goose_completion: {}", error_msg);
            create_error_response(&error_msg)
        }
    }
}

// Helper function to create an error response
unsafe fn create_error_response(error_msg: &str) -> *mut CompletionResponseFFI {
    println!("create_error_response: Creating error response: {}", error_msg);
    let response = Box::new(CompletionResponseFFI {
        content: ptr::null_mut(),
        succeeded: false,
        error_message: string_to_c_char(error_msg),
    });
    println!("create_error_response: Error response created");
    Box::into_raw(response)
}
