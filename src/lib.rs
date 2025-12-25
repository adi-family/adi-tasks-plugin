//! ADI Tasks Plugin
//!
//! Provides CLI commands for task management with dependency tracking.

use abi_stable::std_types::{ROption, RResult, RStr, RString, RVec};
use lib_plugin_abi::{
    PluginContext, PluginError, PluginInfo, PluginVTable, ServiceDescriptor, ServiceError,
    ServiceHandle, ServiceMethod, ServiceVTable, ServiceVersion,
};

/// Plugin-specific CLI service ID
const SERVICE_CLI: &str = "adi.tasks.cli";
use once_cell::sync::OnceCell;
use serde_json::json;
use std::ffi::c_void;
use std::path::PathBuf;

use adi_tasks_core::{CreateTask, TaskId, TaskManager, TaskStatus};

static TASKS: OnceCell<Option<TaskManager>> = OnceCell::new();

// === Plugin VTable Implementation ===

extern "C" fn plugin_info() -> PluginInfo {
    PluginInfo::new("adi.tasks", "ADI Tasks", env!("CARGO_PKG_VERSION"), "core")
        .with_author("ADI Team")
        .with_description("Task management with dependency tracking")
        .with_min_host_version("0.8.0")
}

extern "C" fn plugin_init(ctx: *mut PluginContext) -> i32 {
    let _ = TASKS.set(TaskManager::open_global().ok());

    unsafe {
        let host = (*ctx).host();

        // Register CLI commands service
        let cli_descriptor =
            ServiceDescriptor::new(SERVICE_CLI, ServiceVersion::new(1, 0, 0), "adi.tasks")
                .with_description("CLI commands for task management");

        let cli_handle = ServiceHandle::new(
            SERVICE_CLI,
            ctx as *const c_void,
            &CLI_SERVICE_VTABLE as *const ServiceVTable,
        );

        if let Err(code) = host.register_svc(cli_descriptor, cli_handle) {
            host.error(&format!(
                "Failed to register CLI commands service: {}",
                code
            ));
            return code;
        }

        host.info("ADI Tasks plugin initialized");
    }

    0
}

extern "C" fn plugin_cleanup(_ctx: *mut PluginContext) {}

extern "C" fn handle_message(
    _ctx: *mut PluginContext,
    msg_type: RStr<'_>,
    msg_data: RStr<'_>,
) -> RResult<RString, PluginError> {
    match msg_type.as_str() {
        "set_project_path" => {
            let path = PathBuf::from(msg_data.as_str());
            match TaskManager::open(&path) {
                Ok(_) => RResult::ROk(RString::from("ok")),
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

// === CLI Service VTable ===

static CLI_SERVICE_VTABLE: ServiceVTable = ServiceVTable {
    invoke: cli_invoke,
    list_methods: cli_list_methods,
};

extern "C" fn cli_invoke(
    _handle: *const c_void,
    method: RStr<'_>,
    args: RStr<'_>,
) -> RResult<RString, ServiceError> {
    match method.as_str() {
        "run_command" => {
            let result = run_cli_command(args.as_str());
            match result {
                Ok(output) => RResult::ROk(RString::from(output)),
                Err(e) => RResult::RErr(ServiceError::invocation_error(e)),
            }
        }
        "list_commands" => {
            let commands = json!([
                {"name": "list", "description": "List all tasks", "usage": "list [--status <status>] [--ready] [--blocked]"},
                {"name": "add", "description": "Add a new task", "usage": "add <title> [--description <desc>]"},
                {"name": "show", "description": "Show task details", "usage": "show <id>"},
                {"name": "status", "description": "Update task status", "usage": "status <id> <status>"},
                {"name": "delete", "description": "Delete a task", "usage": "delete <id> [--force]"},
                {"name": "depend", "description": "Add dependency", "usage": "depend <task-id> <depends-on-id>"},
                {"name": "undepend", "description": "Remove dependency", "usage": "undepend <task-id> <depends-on-id>"},
                {"name": "graph", "description": "Show dependency graph", "usage": "graph [--format <text|dot|json>]"},
                {"name": "search", "description": "Search tasks", "usage": "search <query> [--limit <n>]"},
                {"name": "blocked", "description": "Show blocked tasks", "usage": "blocked"},
                {"name": "cycles", "description": "Detect dependency cycles", "usage": "cycles"},
                {"name": "stats", "description": "Show task statistics", "usage": "stats"}
            ]);
            RResult::ROk(RString::from(
                serde_json::to_string(&commands).unwrap_or_default(),
            ))
        }
        _ => RResult::RErr(ServiceError::method_not_found(method.as_str())),
    }
}

extern "C" fn cli_list_methods(_handle: *const c_void) -> RVec<ServiceMethod> {
    vec![
        ServiceMethod::new("run_command").with_description("Run a CLI command"),
        ServiceMethod::new("list_commands").with_description("List available commands"),
    ]
    .into_iter()
    .collect()
}

fn run_cli_command(context_json: &str) -> Result<String, String> {
    let context: serde_json::Value =
        serde_json::from_str(context_json).map_err(|e| format!("Invalid context: {}", e))?;

    let tasks = TASKS
        .get()
        .and_then(|t| t.as_ref())
        .ok_or_else(|| "Tasks not initialized".to_string())?;

    // Parse command and args from context
    let args: Vec<String> = context
        .get("args")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    let subcommand = args.first().map(|s| s.as_str()).unwrap_or("");
    let cmd_args: Vec<&str> = args.iter().skip(1).map(|s| s.as_str()).collect();

    // Parse options from remaining args (--key value format)
    let mut options = serde_json::Map::new();
    let mut i = 0;
    while i < cmd_args.len() {
        if cmd_args[i].starts_with("--") {
            let key = cmd_args[i].trim_start_matches("--");
            if i + 1 < cmd_args.len() && !cmd_args[i + 1].starts_with("--") {
                options.insert(key.to_string(), json!(cmd_args[i + 1]));
                i += 2;
            } else {
                options.insert(key.to_string(), json!(true));
                i += 1;
            }
        } else {
            i += 1;
        }
    }

    // Get positional args (non-option args after subcommand)
    let positional: Vec<&str> = cmd_args
        .iter()
        .filter(|a| !a.starts_with("--"))
        .copied()
        .collect();

    let options_value = serde_json::Value::Object(options);

    match subcommand {
        "list" => cmd_list(tasks, &options_value),
        "add" => cmd_add(tasks, &positional, &options_value),
        "show" => cmd_show(tasks, &positional),
        "status" => cmd_status(tasks, &positional),
        "delete" => cmd_delete(tasks, &positional, &options_value),
        "depend" => cmd_depend(tasks, &positional),
        "undepend" => cmd_undepend(tasks, &positional),
        "graph" => cmd_graph(tasks, &options_value),
        "search" => cmd_search(tasks, &positional, &options_value),
        "blocked" => cmd_blocked(tasks),
        "cycles" => cmd_cycles(tasks),
        "stats" => cmd_stats(tasks),
        "" => {
            let help = "ADI Tasks - Task management with dependency tracking\n\n\
                        Commands:\n  \
                        list     List all tasks\n  \
                        add      Add a new task\n  \
                        show     Show task details\n  \
                        status   Update task status\n  \
                        delete   Delete a task\n  \
                        depend   Add dependency\n  \
                        undepend Remove dependency\n  \
                        graph    Show dependency graph\n  \
                        search   Search tasks\n  \
                        blocked  Show blocked tasks\n  \
                        cycles   Detect dependency cycles\n  \
                        stats    Show task statistics\n\n\
                        Usage: adi run adi.tasks <command> [args]";
            Ok(help.to_string())
        }
        _ => Err(format!("Unknown command: {}", subcommand)),
    }
}

// === Command Implementations ===

fn cmd_list(tasks: &TaskManager, options: &serde_json::Value) -> Result<String, String> {
    let status_filter = options.get("status").and_then(|v| v.as_str());
    let ready = options
        .get("ready")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let blocked = options
        .get("blocked")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let format = options
        .get("format")
        .and_then(|v| v.as_str())
        .unwrap_or("text");

    let task_list = if ready {
        tasks.get_ready().map_err(|e| e.to_string())?
    } else if blocked {
        tasks.get_blocked().map_err(|e| e.to_string())?
    } else if let Some(status_str) = status_filter {
        let status: TaskStatus = status_str
            .parse()
            .map_err(|_| format!("Invalid status: {}", status_str))?;
        tasks.get_by_status(status).map_err(|e| e.to_string())?
    } else {
        tasks.list().map_err(|e| e.to_string())?
    };

    if format == "json" {
        return serde_json::to_string_pretty(&task_list).map_err(|e| e.to_string());
    }

    if task_list.is_empty() {
        return Ok("No tasks found".to_string());
    }

    let mut output = String::new();
    for task in task_list {
        let status_icon = match task.status {
            TaskStatus::Todo => "○",
            TaskStatus::InProgress => "◐",
            TaskStatus::Done => "●",
            TaskStatus::Blocked => "✕",
            TaskStatus::Cancelled => "○",
        };
        let scope = if task.is_global() {
            "[global]"
        } else {
            "[project]"
        };
        output.push_str(&format!(
            "{} #{} {} {}\n",
            status_icon, task.id.0, task.title, scope
        ));
    }
    Ok(output.trim_end().to_string())
}

fn cmd_add(
    tasks: &TaskManager,
    args: &[&str],
    options: &serde_json::Value,
) -> Result<String, String> {
    if args.is_empty() {
        return Err("Missing title. Usage: add <title> [--description <desc>]".to_string());
    }

    let title = args[0];
    let description = options.get("description").and_then(|v| v.as_str());
    let depends_on: Vec<i64> = options
        .get("depends-on")
        .and_then(|v| v.as_str())
        .map(|s| {
            s.split(',')
                .filter_map(|id| id.trim().parse().ok())
                .collect()
        })
        .unwrap_or_default();

    let mut input = CreateTask::new(title);
    if let Some(desc) = description {
        input = input.with_description(desc.to_string());
    }
    if !depends_on.is_empty() {
        input = input.with_dependencies(depends_on.into_iter().map(TaskId).collect());
    }

    let id = tasks.create_task(input).map_err(|e| e.to_string())?;
    Ok(format!("Created task #{}: {}", id.0, title))
}

fn cmd_show(tasks: &TaskManager, args: &[&str]) -> Result<String, String> {
    if args.is_empty() {
        return Err("Missing task ID. Usage: show <id>".to_string());
    }

    let id: i64 = args[0].parse().map_err(|_| "Invalid task ID")?;
    let task_with_deps = tasks
        .get_task_with_dependencies(TaskId(id))
        .map_err(|e| e.to_string())?;
    let task = &task_with_deps.task;

    let mut output = format!("Task #{}\n", task.id.0);
    output.push_str(&format!("  Title: {}\n", task.title));
    output.push_str(&format!("  Status: {:?}\n", task.status));

    if let Some(ref desc) = task.description {
        output.push_str(&format!("  Description: {}\n", desc));
    }

    if let Some(symbol_id) = task.symbol_id {
        output.push_str(&format!("  Linked symbol: #{}\n", symbol_id));
    }

    let scope = if task.is_global() {
        "global"
    } else {
        "project"
    };
    output.push_str(&format!("  Scope: {}\n", scope));

    if !task_with_deps.depends_on.is_empty() {
        output.push_str("\n  Dependencies:\n");
        for dep in &task_with_deps.depends_on {
            output.push_str(&format!("    #{}: {}\n", dep.id.0, dep.title));
        }
    }

    if !task_with_deps.dependents.is_empty() {
        output.push_str("\n  Dependents:\n");
        for dep in &task_with_deps.dependents {
            output.push_str(&format!("    #{}: {}\n", dep.id.0, dep.title));
        }
    }

    Ok(output.trim_end().to_string())
}

fn cmd_status(tasks: &TaskManager, args: &[&str]) -> Result<String, String> {
    if args.len() < 2 {
        return Err("Missing arguments. Usage: status <id> <status>".to_string());
    }

    let id: i64 = args[0].parse().map_err(|_| "Invalid task ID")?;
    let status: TaskStatus = args[1]
        .parse()
        .map_err(|_| format!("Invalid status: {}", args[1]))?;

    tasks
        .update_status(TaskId(id), status)
        .map_err(|e| e.to_string())?;
    Ok(format!("Task #{} status updated to {:?}", id, status))
}

fn cmd_delete(
    tasks: &TaskManager,
    args: &[&str],
    options: &serde_json::Value,
) -> Result<String, String> {
    if args.is_empty() {
        return Err("Missing task ID. Usage: delete <id> [--force]".to_string());
    }

    let id: i64 = args[0].parse().map_err(|_| "Invalid task ID")?;
    let force = options
        .get("force")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let task = tasks.get_task(TaskId(id)).map_err(|e| e.to_string())?;

    if !force {
        return Ok(format!(
            "Delete task #{}: {}?\nUse --force to confirm deletion",
            id, task.title
        ));
    }

    tasks.delete_task(TaskId(id)).map_err(|e| e.to_string())?;
    Ok(format!("Deleted task #{}: {}", id, task.title))
}

fn cmd_depend(tasks: &TaskManager, args: &[&str]) -> Result<String, String> {
    if args.len() < 2 {
        return Err("Missing arguments. Usage: depend <task-id> <depends-on-id>".to_string());
    }

    let task_id: i64 = args[0].parse().map_err(|_| "Invalid task ID")?;
    let depends_on: i64 = args[1].parse().map_err(|_| "Invalid depends-on ID")?;

    tasks
        .add_dependency(TaskId(task_id), TaskId(depends_on))
        .map_err(|e| e.to_string())?;
    Ok(format!(
        "Task #{} now depends on task #{}",
        task_id, depends_on
    ))
}

fn cmd_undepend(tasks: &TaskManager, args: &[&str]) -> Result<String, String> {
    if args.len() < 2 {
        return Err("Missing arguments. Usage: undepend <task-id> <depends-on-id>".to_string());
    }

    let task_id: i64 = args[0].parse().map_err(|_| "Invalid task ID")?;
    let depends_on: i64 = args[1].parse().map_err(|_| "Invalid depends-on ID")?;

    tasks
        .remove_dependency(TaskId(task_id), TaskId(depends_on))
        .map_err(|e| e.to_string())?;
    Ok(format!(
        "Removed dependency: #{} -> #{}",
        task_id, depends_on
    ))
}

fn cmd_graph(tasks: &TaskManager, options: &serde_json::Value) -> Result<String, String> {
    let format = options
        .get("format")
        .and_then(|v| v.as_str())
        .unwrap_or("text");
    let all_tasks = tasks.list().map_err(|e| e.to_string())?;

    if format == "json" {
        let mut graph_data = Vec::new();
        for task in &all_tasks {
            let deps = tasks.get_dependencies(task.id).map_err(|e| e.to_string())?;
            graph_data.push(json!({
                "task": task,
                "dependencies": deps.iter().map(|d| d.id.0).collect::<Vec<_>>()
            }));
        }
        return serde_json::to_string_pretty(&graph_data).map_err(|e| e.to_string());
    }

    if format == "dot" {
        let mut output = String::from("digraph tasks {\n  rankdir=LR;\n");
        for task in &all_tasks {
            let label = task.title.replace('"', "\\\"");
            let color = match task.status {
                TaskStatus::Done => "green",
                TaskStatus::InProgress => "blue",
                TaskStatus::Blocked => "red",
                TaskStatus::Cancelled => "gray",
                TaskStatus::Todo => "black",
            };
            output.push_str(&format!(
                "  {} [label=\"{}\" color=\"{}\"];\n",
                task.id.0, label, color
            ));

            let deps = tasks.get_dependencies(task.id).map_err(|e| e.to_string())?;
            for dep in deps {
                output.push_str(&format!("  {} -> {};\n", task.id.0, dep.id.0));
            }
        }
        output.push_str("}\n");
        return Ok(output);
    }

    // Text format
    if all_tasks.is_empty() {
        return Ok("No tasks found".to_string());
    }

    let mut output = String::from("Task Dependency Graph\n\n");
    for task in &all_tasks {
        let status_icon = match task.status {
            TaskStatus::Todo => "○",
            TaskStatus::InProgress => "◐",
            TaskStatus::Done => "●",
            TaskStatus::Blocked => "✕",
            TaskStatus::Cancelled => "○",
        };
        output.push_str(&format!("{} #{} {}\n", status_icon, task.id.0, task.title));

        let deps = tasks.get_dependencies(task.id).map_err(|e| e.to_string())?;
        for (i, dep) in deps.iter().enumerate() {
            let prefix = if i == deps.len() - 1 {
                "  └─"
            } else {
                "  ├─"
            };
            output.push_str(&format!(
                "{} depends on #{}: {}\n",
                prefix, dep.id.0, dep.title
            ));
        }
    }
    Ok(output.trim_end().to_string())
}

fn cmd_search(
    tasks: &TaskManager,
    args: &[&str],
    options: &serde_json::Value,
) -> Result<String, String> {
    if args.is_empty() {
        return Err("Missing query. Usage: search <query> [--limit <n>]".to_string());
    }

    let query = args[0];
    let limit = options
        .get("limit")
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse().ok())
        .unwrap_or(10usize);

    let results = tasks.search(query, limit).map_err(|e| e.to_string())?;

    if results.is_empty() {
        return Ok("No tasks found".to_string());
    }

    let mut output = format!("Found {} results for \"{}\":\n\n", results.len(), query);
    for task in results {
        let status_icon = match task.status {
            TaskStatus::Todo => "○",
            TaskStatus::InProgress => "◐",
            TaskStatus::Done => "●",
            TaskStatus::Blocked => "✕",
            TaskStatus::Cancelled => "○",
        };
        output.push_str(&format!("{} #{} {}\n", status_icon, task.id.0, task.title));
    }
    Ok(output.trim_end().to_string())
}

fn cmd_blocked(tasks: &TaskManager) -> Result<String, String> {
    let blocked = tasks.get_blocked().map_err(|e| e.to_string())?;

    if blocked.is_empty() {
        return Ok("No blocked tasks".to_string());
    }

    let mut output = String::from("Blocked Tasks\n\n");
    for task in blocked {
        output.push_str(&format!("✕ #{} {}\n", task.id.0, task.title));

        let blockers = tasks.get_dependencies(task.id).map_err(|e| e.to_string())?;
        let incomplete_blockers: Vec<_> = blockers
            .iter()
            .filter(|t| !t.status.is_complete())
            .collect();

        for blocker in incomplete_blockers {
            output.push_str(&format!(
                "  └─ blocked by #{}: {} ({:?})\n",
                blocker.id.0, blocker.title, blocker.status
            ));
        }
    }
    Ok(output.trim_end().to_string())
}

fn cmd_cycles(tasks: &TaskManager) -> Result<String, String> {
    let cycles = tasks.detect_cycles().map_err(|e| e.to_string())?;

    if cycles.is_empty() {
        return Ok("No circular dependencies detected".to_string());
    }

    let mut output = format!("Found {} circular dependencies:\n\n", cycles.len());
    for (i, cycle) in cycles.iter().enumerate() {
        output.push_str(&format!("  Cycle {}: ", i + 1));
        let cycle_str = cycle
            .iter()
            .map(|id| format!("#{}", id.0))
            .collect::<Vec<_>>()
            .join(" -> ");
        output.push_str(&format!(
            "{} -> #{}\n",
            cycle_str,
            cycle.first().map(|id| id.0).unwrap_or(0)
        ));
    }
    Ok(output.trim_end().to_string())
}

fn cmd_stats(tasks: &TaskManager) -> Result<String, String> {
    let status = tasks.status().map_err(|e| e.to_string())?;

    let mut output = String::from("Task Statistics\n\n");
    output.push_str(&format!("  Total tasks:     {}\n", status.total_tasks));
    output.push_str(&format!("  Todo:            {}\n", status.todo_count));
    output.push_str(&format!(
        "  In Progress:     {}\n",
        status.in_progress_count
    ));
    output.push_str(&format!("  Done:            {}\n", status.done_count));
    output.push_str(&format!("  Blocked:         {}\n", status.blocked_count));
    output.push_str(&format!("  Cancelled:       {}\n", status.cancelled_count));
    output.push_str(&format!(
        "\n  Dependencies:    {}\n",
        status.total_dependencies
    ));

    if status.has_cycles {
        output.push_str("  Cycles:          Yes (run 'cycles' to see)\n");
    } else {
        output.push_str("  Cycles:          None\n");
    }

    Ok(output.trim_end().to_string())
}
