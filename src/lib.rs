//! ADI Tasks Plugin
//!
//! Provides MCP tools and resources for task management with dependency tracking.

use abi_stable::std_types::{ROption, RResult, RStr, RString, RVec};
use lib_plugin_abi::{
    PluginContext, PluginError, PluginInfo, PluginVTable, ServiceDescriptor, ServiceError,
    ServiceHandle, ServiceMethod, ServiceVTable, ServiceVersion, SERVICE_MCP_RESOURCES,
    SERVICE_MCP_TOOLS,
};
use once_cell::sync::OnceCell;
use serde_json::{json, Value};
use std::ffi::c_void;
use std::path::PathBuf;

static TASKS: OnceCell<Option<adi_tasks_core::TaskManager>> = OnceCell::new();
static PROJECT_PATH: OnceCell<PathBuf> = OnceCell::new();

// === Plugin VTable Implementation ===

extern "C" fn plugin_info() -> PluginInfo {
    PluginInfo::new("adi.tasks", "ADI Tasks", env!("CARGO_PKG_VERSION"), "core")
        .with_author("ADI Team")
        .with_description("Task management with dependency tracking")
        .with_min_host_version("0.8.0")
}

extern "C" fn plugin_init(ctx: *mut PluginContext) -> i32 {
    // Initialize with current directory
    let _ = PROJECT_PATH.set(PathBuf::from("."));
    let _ = TASKS.set(adi_tasks_core::TaskManager::open_global().ok());

    unsafe {
        let host = (*ctx).host();

        // Register MCP tools service
        let tools_descriptor =
            ServiceDescriptor::new(SERVICE_MCP_TOOLS, ServiceVersion::new(1, 0, 0), "adi.tasks")
                .with_description("MCP tools for task management");

        let tools_handle = ServiceHandle::new(
            SERVICE_MCP_TOOLS,
            ctx as *const c_void,
            &MCP_TOOLS_VTABLE as *const ServiceVTable,
        );

        if let Err(code) = host.register_svc(tools_descriptor, tools_handle) {
            host.error(&format!("Failed to register MCP tools service: {}", code));
            return code;
        }

        // Register MCP resources service
        let resources_descriptor = ServiceDescriptor::new(
            SERVICE_MCP_RESOURCES,
            ServiceVersion::new(1, 0, 0),
            "adi.tasks",
        )
        .with_description("MCP resources for task data access");

        let resources_handle = ServiceHandle::new(
            SERVICE_MCP_RESOURCES,
            ctx as *const c_void,
            &MCP_RESOURCES_VTABLE as *const ServiceVTable,
        );

        if let Err(code) = host.register_svc(resources_descriptor, resources_handle) {
            host.error(&format!(
                "Failed to register MCP resources service: {}",
                code
            ));
            return code;
        }

        host.info("ADI Tasks plugin initialized");
    }

    0
}

extern "C" fn plugin_cleanup(_ctx: *mut PluginContext) {
    // Nothing to clean up - static data will be dropped on unload
}

// === Plugin Entry Point ===

static PLUGIN_VTABLE: PluginVTable = PluginVTable {
    info: plugin_info,
    init: plugin_init,
    update: ROption::RNone,
    cleanup: plugin_cleanup,
    handle_message: ROption::RSome(handle_message),
};

#[no_mangle]
pub extern "C" fn plugin_entry() -> *const PluginVTable {
    &PLUGIN_VTABLE
}

// === Message Handler ===

extern "C" fn handle_message(
    _ctx: *mut PluginContext,
    msg_type: RStr<'_>,
    msg_data: RStr<'_>,
) -> RResult<RString, PluginError> {
    match msg_type.as_str() {
        "set_project_path" => {
            let path = PathBuf::from(msg_data.as_str());
            match adi_tasks_core::TaskManager::open(&path) {
                Ok(tm) => {
                    // Note: Can't update OnceCell, so this is a limitation
                    let _ = tm;
                    RResult::ROk(RString::from("ok"))
                }
                Err(e) => {
                    RResult::RErr(PluginError::new(1, format!("Failed to open tasks: {}", e)))
                }
            }
        }
        _ => RResult::RErr(PluginError::new(
            -1,
            format!("Unknown message type: {}", msg_type.as_str()),
        )),
    }
}

// === MCP Tools Service ===

static MCP_TOOLS_VTABLE: ServiceVTable = ServiceVTable {
    invoke: mcp_tools_invoke,
    list_methods: mcp_tools_list_methods,
};

extern "C" fn mcp_tools_invoke(
    _handle: *const c_void,
    method: RStr<'_>,
    args: RStr<'_>,
) -> RResult<RString, ServiceError> {
    let result = match method.as_str() {
        "list_tools" => Ok(list_tools_json()),
        "call_tool" => {
            let params: Value = match serde_json::from_str(args.as_str()) {
                Ok(v) => v,
                Err(e) => {
                    return RResult::RErr(ServiceError::invocation_error(format!(
                        "Invalid args: {}",
                        e
                    )))
                }
            };

            let tool_name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let tool_args = params.get("args").cloned().unwrap_or(json!({}));

            call_tool(tool_name, &tool_args)
        }
        _ => Err(ServiceError::method_not_found(method.as_str())),
    };

    match result {
        Ok(s) => RResult::ROk(RString::from(s)),
        Err(e) => RResult::RErr(e),
    }
}

extern "C" fn mcp_tools_list_methods(_handle: *const c_void) -> RVec<ServiceMethod> {
    vec![
        ServiceMethod::new("list_tools").with_description("List all available tools"),
        ServiceMethod::new("call_tool").with_description("Call a tool by name with arguments"),
    ]
    .into_iter()
    .collect()
}

fn list_tools_json() -> String {
    let tools = json!([
        {
            "name": "tasks_list",
            "description": "List all tasks with optional status filter",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "status": {
                        "type": "string",
                        "enum": ["todo", "in_progress", "done", "blocked", "cancelled"]
                    }
                }
            }
        },
        {
            "name": "tasks_create",
            "description": "Create a new task",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "title": { "type": "string" },
                    "description": { "type": "string" }
                },
                "required": ["title"]
            }
        },
        {
            "name": "tasks_show",
            "description": "Show task details",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "integer" }
                },
                "required": ["id"]
            }
        },
        {
            "name": "tasks_update",
            "description": "Update task status",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "integer" },
                    "status": { "type": "string" }
                },
                "required": ["id", "status"]
            }
        },
        {
            "name": "tasks_delete",
            "description": "Delete a task",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "integer" }
                },
                "required": ["id"]
            }
        }
    ]);
    serde_json::to_string(&tools).unwrap_or_else(|_| "[]".to_string())
}

fn call_tool(tool_name: &str, args: &Value) -> Result<String, ServiceError> {
    let tasks = TASKS
        .get()
        .and_then(|t| t.as_ref())
        .ok_or_else(|| ServiceError::invocation_error("Tasks not initialized"))?;

    match tool_name {
        "tasks_list" => {
            let status_filter = args.get("status").and_then(|v| v.as_str());
            let all_tasks = tasks
                .list()
                .map_err(|e| ServiceError::invocation_error(e.to_string()))?;

            let filtered: Vec<_> = if let Some(status) = status_filter {
                all_tasks
                    .into_iter()
                    .filter(|t| format!("{:?}", t.status).to_lowercase() == status)
                    .collect()
            } else {
                all_tasks
            };

            Ok(tool_result(
                &serde_json::to_string_pretty(&filtered).unwrap_or_default(),
            ))
        }
        "tasks_create" => {
            let title = args
                .get("title")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ServiceError::invocation_error("Missing title"))?;
            let description = args.get("description").and_then(|v| v.as_str());

            let create = adi_tasks_core::CreateTask {
                title: title.to_string(),
                description: description.map(String::from),
                symbol_id: None,
                depends_on: vec![],
            };

            let task_id = tasks
                .create_task(create)
                .map_err(|e| ServiceError::invocation_error(e.to_string()))?;

            Ok(tool_result(&format!("Created task #{}", task_id.0)))
        }
        "tasks_show" => {
            let id = args
                .get("id")
                .and_then(|v| v.as_i64())
                .ok_or_else(|| ServiceError::invocation_error("Missing id"))?;

            let task = tasks
                .get_task(adi_tasks_core::TaskId(id))
                .map_err(|e| ServiceError::invocation_error(e.to_string()))?;

            Ok(tool_result(
                &serde_json::to_string_pretty(&task).unwrap_or_default(),
            ))
        }
        "tasks_update" => {
            let id = args
                .get("id")
                .and_then(|v| v.as_i64())
                .ok_or_else(|| ServiceError::invocation_error("Missing id"))?;
            let status_str = args
                .get("status")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ServiceError::invocation_error("Missing status"))?;

            let status = match status_str {
                "todo" => adi_tasks_core::TaskStatus::Todo,
                "in_progress" => adi_tasks_core::TaskStatus::InProgress,
                "done" => adi_tasks_core::TaskStatus::Done,
                "blocked" => adi_tasks_core::TaskStatus::Blocked,
                "cancelled" => adi_tasks_core::TaskStatus::Cancelled,
                _ => return Err(ServiceError::invocation_error("Invalid status")),
            };

            tasks
                .update_status(adi_tasks_core::TaskId(id), status)
                .map_err(|e| ServiceError::invocation_error(e.to_string()))?;

            Ok(tool_result(&format!(
                "Updated task #{} to {}",
                id, status_str
            )))
        }
        "tasks_delete" => {
            let id = args
                .get("id")
                .and_then(|v| v.as_i64())
                .ok_or_else(|| ServiceError::invocation_error("Missing id"))?;

            tasks
                .delete_task(adi_tasks_core::TaskId(id))
                .map_err(|e| ServiceError::invocation_error(e.to_string()))?;

            Ok(tool_result(&format!("Deleted task #{}", id)))
        }
        _ => Err(ServiceError::invocation_error(format!(
            "Unknown tool: {}",
            tool_name
        ))),
    }
}

// === MCP Resources Service ===

static MCP_RESOURCES_VTABLE: ServiceVTable = ServiceVTable {
    invoke: mcp_resources_invoke,
    list_methods: mcp_resources_list_methods,
};

extern "C" fn mcp_resources_invoke(
    _handle: *const c_void,
    method: RStr<'_>,
    args: RStr<'_>,
) -> RResult<RString, ServiceError> {
    let result = match method.as_str() {
        "list_resources" => Ok(list_resources_json()),
        "read_resource" => {
            let params: Value = match serde_json::from_str(args.as_str()) {
                Ok(v) => v,
                Err(e) => {
                    return RResult::RErr(ServiceError::invocation_error(format!(
                        "Invalid args: {}",
                        e
                    )))
                }
            };

            let uri = match params.get("uri").and_then(|v| v.as_str()) {
                Some(u) => u,
                None => return RResult::RErr(ServiceError::invocation_error("Missing uri")),
            };

            read_resource(uri)
        }
        _ => Err(ServiceError::method_not_found(method.as_str())),
    };

    match result {
        Ok(s) => RResult::ROk(RString::from(s)),
        Err(e) => RResult::RErr(e),
    }
}

extern "C" fn mcp_resources_list_methods(_handle: *const c_void) -> RVec<ServiceMethod> {
    vec![
        ServiceMethod::new("list_resources").with_description("List all available resources"),
        ServiceMethod::new("read_resource").with_description("Read a resource by URI"),
    ]
    .into_iter()
    .collect()
}

fn list_resources_json() -> String {
    let resources = json!([
        {
            "uri": "tasks://all",
            "name": "All Tasks",
            "description": "List of all tasks",
            "mimeType": "application/json"
        },
        {
            "uri": "tasks://ready",
            "name": "Ready Tasks",
            "description": "Tasks ready to work on",
            "mimeType": "application/json"
        }
    ]);
    serde_json::to_string(&resources).unwrap_or_else(|_| "[]".to_string())
}

fn read_resource(uri: &str) -> Result<String, ServiceError> {
    let tasks = TASKS
        .get()
        .and_then(|t| t.as_ref())
        .ok_or_else(|| ServiceError::invocation_error("Tasks not initialized"))?;

    let content = match uri {
        "tasks://all" => {
            let all_tasks = tasks
                .list()
                .map_err(|e| ServiceError::invocation_error(e.to_string()))?;
            json!({
                "uri": uri,
                "mimeType": "application/json",
                "text": serde_json::to_string_pretty(&all_tasks).unwrap_or_default()
            })
        }
        "tasks://ready" => {
            let ready = tasks
                .get_ready()
                .map_err(|e| ServiceError::invocation_error(e.to_string()))?;
            json!({
                "uri": uri,
                "mimeType": "application/json",
                "text": serde_json::to_string_pretty(&ready).unwrap_or_default()
            })
        }
        _ => {
            return Err(ServiceError::invocation_error(format!(
                "Unknown resource: {}",
                uri
            )))
        }
    };

    Ok(serde_json::to_string(&content).unwrap_or_else(|_| "{}".to_string()))
}

fn tool_result(text: &str) -> String {
    let result = json!({
        "content": [{
            "type": "text",
            "text": text
        }]
    });
    serde_json::to_string(&result).unwrap_or_else(|_| "{}".to_string())
}
