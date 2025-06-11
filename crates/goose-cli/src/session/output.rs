use bat::WrappingMode;
use console::{style, Color};
use goose::config::Config;
use goose::message::{Message, MessageContent, ToolRequest, ToolResponse};
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use mcp_core::prompt::PromptArgument;
use mcp_core::tool::ToolCall;
use serde_json::Value;
use std::cell::RefCell;
use std::collections::HashMap;
use std::io::{self, Error, Write};
use std::path::Path;
use std::sync::{atomic, Arc};
use std::time::Duration;

// Re-export theme for use in main
#[derive(Clone, Copy)]
pub enum Theme {
    Light,
    Dark,
    Ansi,
}

impl Theme {
    fn as_str(&self) -> &'static str {
        match self {
            Theme::Light => "GitHub",
            Theme::Dark => "zenburn",
            Theme::Ansi => "base16",
        }
    }

    fn from_config_str(val: &str) -> Self {
        if val.eq_ignore_ascii_case("light") {
            Theme::Light
        } else if val.eq_ignore_ascii_case("ansi") {
            Theme::Ansi
        } else {
            Theme::Dark
        }
    }

    fn as_config_string(&self) -> String {
        match self {
            Theme::Light => "light".to_string(),
            Theme::Dark => "dark".to_string(),
            Theme::Ansi => "ansi".to_string(),
        }
    }
}

thread_local! {
    static CURRENT_THEME: RefCell<Theme> = RefCell::new(
        std::env::var("GOOSE_CLI_THEME").ok()
            .map(|val| Theme::from_config_str(&val))
            .unwrap_or_else(||
                Config::global().get_param::<String>("GOOSE_CLI_THEME").ok()
                    .map(|val| Theme::from_config_str(&val))
                    .unwrap_or(Theme::Dark)
            )
    );
}

pub fn set_theme(theme: Theme) {
    let config = Config::global();
    config
        .set_param("GOOSE_CLI_THEME", Value::String(theme.as_config_string()))
        .expect("Failed to set theme");
    CURRENT_THEME.with(|t| *t.borrow_mut() = theme);
}

pub fn get_theme() -> Theme {
    CURRENT_THEME.with(|t| *t.borrow())
}

// Simple wrapper around spinner to manage its state
#[derive(Default)]
pub struct ThinkingIndicator {
    spinner: Option<cliclack::ProgressBar>,
    worm_thread: Option<std::thread::JoinHandle<()>>,
    should_stop: Option<Arc<atomic::AtomicBool>>,
}

impl ThinkingIndicator {
    pub fn show(&mut self) {
        // Start the animated worm progress indicator
        let should_stop = Arc::new(atomic::AtomicBool::new(false));
        self.should_stop = Some(should_stop.clone());

        let worm_thread = std::thread::spawn(move || {
            let worm_chars = ["🐛", "🪱", "🐍", "🪱"];
            let mut frame = 0;

            while !should_stop.load(atomic::Ordering::Relaxed) {
                // Clear the line and show the worm
                print!(
                    "\r{} {} Thinking...",
                    style("🪿").bold(),
                    style(worm_chars[frame % worm_chars.len()]).green().bold()
                );
                io::stdout().flush().unwrap_or(());

                frame += 1;
                std::thread::sleep(std::time::Duration::from_millis(300));
            }

            // Clear the line when done
            print!("\r{}", " ".repeat(50));
            print!("\r");
            io::stdout().flush().unwrap_or(());
        });

        self.worm_thread = Some(worm_thread);
    }

    pub fn hide(&mut self) {
        // Stop the worm animation
        if let Some(should_stop) = &self.should_stop {
            should_stop.store(true, atomic::Ordering::Relaxed);
        }

        if let Some(thread) = self.worm_thread.take() {
            let _ = thread.join();
        }

        self.should_stop = None;

        // Also handle the old spinner if it exists
        if let Some(spinner) = self.spinner.take() {
            spinner.stop("");
        }
    }
}

#[derive(Debug, Clone)]
pub struct PromptInfo {
    pub name: String,
    pub description: Option<String>,
    pub arguments: Option<Vec<PromptArgument>>,
    pub extension: Option<String>,
}

// Global thinking indicator
thread_local! {
    static THINKING: RefCell<ThinkingIndicator> = RefCell::new(ThinkingIndicator::default());
}

pub fn show_thinking() {
    THINKING.with(|t| t.borrow_mut().show());
}

pub fn hide_thinking() {
    THINKING.with(|t| t.borrow_mut().hide());
}

pub fn set_thinking_message(s: &String) {
    THINKING.with(|t| {
        if let Some(spinner) = t.borrow_mut().spinner.as_mut() {
            spinner.set_message(s);
        }
    });
}

pub fn render_message(message: &Message, debug: bool) {
    let theme = get_theme();

    for content in &message.content {
        match content {
            MessageContent::Text(text) => print_markdown(&text.text, theme),
            MessageContent::ToolRequest(req) => render_tool_request(req, theme, debug),
            MessageContent::ToolResponse(resp) => render_tool_response(resp, theme, debug),
            MessageContent::Image(image) => {
                println!("Image: [data: {}, type: {}]", image.data, image.mime_type);
            }
            MessageContent::Thinking(thinking) => {
                if std::env::var("GOOSE_CLI_SHOW_THINKING").is_ok() {
                    println!("\n{}", style("Thinking:").dim().italic());
                    print_markdown(&thinking.thinking, theme);
                }
            }
            MessageContent::RedactedThinking(_) => {
                // For redacted thinking, print thinking was redacted
                println!("\n{}", style("Thinking:").dim().italic());
                print_markdown("Thinking was redacted", theme);
            }
            _ => {
                println!("WARNING: Message content type could not be rendered");
            }
        }
    }
    println!();
}

pub fn render_text(text: &str, color: Option<Color>, dim: bool) {
    render_text_no_newlines(format!("\n{}\n\n", text).as_str(), color, dim);
}

pub fn render_text_no_newlines(text: &str, color: Option<Color>, dim: bool) {
    let mut styled_text = style(text);
    if dim {
        styled_text = styled_text.dim();
    }
    if let Some(color) = color {
        styled_text = styled_text.fg(color);
    } else {
        styled_text = styled_text.green();
    }
    print!("{}", styled_text);
}

pub fn render_enter_plan_mode() {
    println!(
        "\n{} {}\n",
        style("Entering plan mode.").green().bold(),
        style("You can provide instructions to create a plan and then act on it. To exit early, type /endplan")
            .green()
            .dim()
    );
}

pub fn render_act_on_plan() {
    println!(
        "\n{}\n",
        style("Exiting plan mode and acting on the above plan")
            .green()
            .bold(),
    );
}

pub fn render_exit_plan_mode() {
    println!("\n{}\n", style("Exiting plan mode.").green().bold());
}

pub fn goose_mode_message(text: &str) {
    println!("\n{}", style(text).yellow(),);
}

fn render_tool_request(req: &ToolRequest, theme: Theme, debug: bool) {
    match &req.tool_call {
        Ok(call) => match call.name.as_str() {
            "developer__text_editor" => render_text_editor_request(call, debug),
            "developer__shell" => render_shell_request(call, debug),
            _ => render_default_request(call, debug),
        },
        Err(e) => print_markdown(&e.to_string(), theme),
    }
}

fn render_tool_response(resp: &ToolResponse, theme: Theme, debug: bool) {
    let config = Config::global();

    match &resp.tool_result {
        Ok(contents) => {
            for content in contents {
                if let Some(audience) = content.audience() {
                    if !audience.contains(&mcp_core::role::Role::User) {
                        continue;
                    }
                }

                let min_priority = config
                    .get_param::<f32>("GOOSE_CLI_MIN_PRIORITY")
                    .ok()
                    .unwrap_or(0.5);

                if content
                    .priority()
                    .is_some_and(|priority| priority < min_priority)
                    || (content.priority().is_none() && !debug)
                {
                    continue;
                }

                if debug {
                    println!("{:#?}", content);
                } else if let mcp_core::content::Content::Text(text) = content {
                    print_markdown(&text.text, theme);
                }
            }
        }
        Err(e) => print_markdown(&e.to_string(), theme),
    }
}

pub fn render_error(message: &str) {
    println!("\n  {} {}\n", style("error:").red().bold(), message);
}

pub fn render_prompts(prompts: &HashMap<String, Vec<String>>) {
    println!();
    for (extension, prompts) in prompts {
        println!(" {}", style(extension).green());
        for prompt in prompts {
            println!("  - {}", style(prompt).cyan());
        }
    }
    println!();
}

pub fn render_prompt_info(info: &PromptInfo) {
    println!();

    if let Some(ext) = &info.extension {
        println!(" {}: {}", style("Extension").green(), ext);
    }

    println!(" Prompt: {}", style(&info.name).cyan().bold());

    if let Some(desc) = &info.description {
        println!("\n {}", desc);
    }

    if let Some(args) = &info.arguments {
        println!("\n Arguments:");
        for arg in args {
            let required = arg.required.unwrap_or(false);
            let req_str = if required {
                style("(required)").red()
            } else {
                style("(optional)").dim()
            };

            println!(
                "  {} {} {}",
                style(&arg.name).yellow(),
                req_str,
                arg.description.as_deref().unwrap_or("")
            );
        }
    }
    println!();
}

pub fn render_extension_success(name: &str) {
    println!();
    println!(
        "  {} extension `{}`",
        style("added").green(),
        style(name).cyan(),
    );
    println!();
}

pub fn render_extension_error(name: &str, error: &str) {
    println!();
    println!(
        "  {} to add extension {}",
        style("failed").red(),
        style(name).red()
    );
    println!();
    println!("{}", style(error).dim());
    println!();
}

pub fn render_builtin_success(names: &str) {
    println!();
    println!(
        "  {} builtin{}: {}",
        style("added").green(),
        if names.contains(',') { "s" } else { "" },
        style(names).cyan()
    );
    println!();
}

pub fn render_builtin_error(names: &str, error: &str) {
    println!();
    println!(
        "  {} to add builtin{}: {}",
        style("failed").red(),
        if names.contains(',') { "s" } else { "" },
        style(names).red()
    );
    println!();
    println!("{}", style(error).dim());
    println!();
}

fn render_text_editor_request(call: &ToolCall, debug: bool) {
    print_tool_header(call);

    let content_width = 77;

    // Print path first with special formatting
    if let Some(Value::String(path)) = call.arguments.get("path") {
        let path_line_content = format!("path: {}", shorten_path(path, debug));
        let path_padding = calculate_padding(&path_line_content, content_width);
        println!(
            "│ {}: {}{}│",
            style("path").dim(),
            style(shorten_path(path, debug)).green(),
            " ".repeat(path_padding)
        );
    }

    // Print other arguments normally, excluding path
    if let Some(args) = call.arguments.as_object() {
        let mut other_args = serde_json::Map::new();
        for (k, v) in args {
            if k != "path" {
                other_args.insert(k.clone(), v.clone());
            }
        }
        print_params_boxed(&Value::Object(other_args), 0, debug);
    }
    println!("╰─────────────────────────────────────────────────────────────────────────────╯");
    println!();
}

fn render_shell_request(call: &ToolCall, debug: bool) {
    print_tool_header(call);

    let content_width = 77;

    match call.arguments.get("command") {
        Some(Value::String(s)) => {
            let command_line_content = format!("command: {}", s);
            let command_padding = calculate_padding(&command_line_content, content_width);
            println!(
                "│ {}: {}{}│",
                style("command").dim(),
                style(s).green(),
                " ".repeat(command_padding)
            );
        }
        _ => print_params_boxed(&call.arguments, 0, debug),
    }
    println!("╰─────────────────────────────────────────────────────────────────────────────╯");
    println!();
}

fn render_default_request(call: &ToolCall, debug: bool) {
    print_tool_header(call);
    print_params_boxed(&call.arguments, 0, debug);
    println!("╰─────────────────────────────────────────────────────────────────────────────╯");
    println!();
}

// Helper functions

fn print_tool_header(call: &ToolCall) {
    let parts: Vec<_> = call.name.rsplit("__").collect();
    let extension_name = parts.first().unwrap_or(&"unknown");
    let tool_name = parts
        .split_first()
        .map(|(_, s)| s.iter().rev().copied().collect::<Vec<_>>().join("__"))
        .unwrap_or_else(|| "unknown".to_string());

    println!();
    println!(
        "╭─ {} ─────────────────────────────────────────────────────────────────────────╮",
        style("Tool Call").bold()
    );

    let content_width = 77;
    let header_line_content = format!("🔧 {} → {}", extension_name, tool_name);
    let header_padding = calculate_padding(&header_line_content, content_width);

    println!(
        "│ {} {} {} {}{}│",
        style("🔧").bold(),
        style(extension_name).magenta().bold(),
        style("→").dim(),
        style(&tool_name).cyan().bold(),
        " ".repeat(header_padding)
    );
    println!("├─────────────────────────────────────────────────────────────────────────────┤");
}

// Respect NO_COLOR, as https://crates.io/crates/console already does
pub fn env_no_color() -> bool {
    // if NO_COLOR is defined at all disable colors
    std::env::var_os("NO_COLOR").is_none()
}

fn print_markdown(content: &str, theme: Theme) {
    bat::PrettyPrinter::new()
        .input(bat::Input::from_bytes(content.as_bytes()))
        .theme(theme.as_str())
        .colored_output(env_no_color())
        .language("Markdown")
        .wrapping_mode(WrappingMode::NoWrapping(true))
        .print()
        .unwrap();
}

const INDENT: &str = "    ";

fn get_tool_params_max_length() -> usize {
    Config::global()
        .get_param::<usize>("GOOSE_CLI_TOOL_PARAMS_TRUNCATION_MAX_LENGTH")
        .ok()
        .unwrap_or(40)
}

fn print_params_boxed(value: &Value, depth: usize, debug: bool) {
    let indent = "│ ";
    let content_width = 77;

    match value {
        Value::Object(map) => {
            for (key, val) in map {
                match val {
                    Value::Object(_) => {
                        let nested_line_content = format!("{}:", key);
                        let nested_padding =
                            calculate_padding(&nested_line_content, content_width - indent.len());
                        println!(
                            "{}{}{}│",
                            indent,
                            style(key).dim(),
                            " ".repeat(nested_padding)
                        );
                        print_params_boxed(val, depth + 1, debug);
                    }
                    Value::Array(arr) => {
                        let array_line_content = format!("{}:", key);
                        let array_padding =
                            calculate_padding(&array_line_content, content_width - indent.len());
                        println!(
                            "{}{}:{}│",
                            indent,
                            style(key).dim(),
                            " ".repeat(array_padding)
                        );
                        for item in arr.iter() {
                            let dash_line_content = "- ";
                            let dash_padding =
                                calculate_padding(dash_line_content, content_width - indent.len());
                            println!("{}- {}│", indent, " ".repeat(dash_padding));
                            print_params_boxed(item, depth + 2, debug);
                        }
                    }
                    Value::String(s) => {
                        if !debug && s.len() > get_tool_params_max_length() {
                            let truncated_line_content = format!("{}: ...", key);
                            let truncated_padding = calculate_padding(
                                &truncated_line_content,
                                content_width - indent.len(),
                            );
                            println!(
                                "{}{}: {}{}│",
                                indent,
                                style(key).dim(),
                                style("...").dim(),
                                " ".repeat(truncated_padding)
                            );
                        } else {
                            let string_line_content = format!("{}: {}", key, s);
                            let string_padding = calculate_padding(
                                &string_line_content,
                                content_width - indent.len(),
                            );
                            println!(
                                "{}{}: {}{}│",
                                indent,
                                style(key).dim(),
                                style(s).green(),
                                " ".repeat(string_padding)
                            );
                        }
                    }
                    Value::Number(n) => {
                        let number_line_content = format!("{}: {}", key, n);
                        let number_padding =
                            calculate_padding(&number_line_content, content_width - indent.len());
                        println!(
                            "{}{}: {}{}│",
                            indent,
                            style(key).dim(),
                            style(n).blue(),
                            " ".repeat(number_padding)
                        );
                    }
                    Value::Bool(b) => {
                        let bool_line_content = format!("{}: {}", key, b);
                        let bool_padding =
                            calculate_padding(&bool_line_content, content_width - indent.len());
                        println!(
                            "{}{}: {}{}│",
                            indent,
                            style(key).dim(),
                            style(b).blue(),
                            " ".repeat(bool_padding)
                        );
                    }
                    Value::Null => {
                        let null_line_content = format!("{}: null", key);
                        let null_padding =
                            calculate_padding(&null_line_content, content_width - indent.len());
                        println!(
                            "{}{}: {}{}│",
                            indent,
                            style(key).dim(),
                            style("null").dim(),
                            " ".repeat(null_padding)
                        );
                    }
                }
            }
        }
        Value::String(s) => {
            if !debug && s.len() > get_tool_params_max_length() {
                let redacted_content = format!("[REDACTED: {} chars]", s.len());
                let redacted_padding =
                    calculate_padding(&redacted_content, content_width - indent.len());
                println!(
                    "{}{}{}│",
                    indent,
                    style(redacted_content).yellow(),
                    " ".repeat(redacted_padding)
                );
            } else {
                let string_content = s;
                let string_padding =
                    calculate_padding(string_content, content_width - indent.len());
                println!(
                    "{}{}{}│",
                    indent,
                    style(s).green(),
                    " ".repeat(string_padding)
                );
            }
        }
        _ => {
            // Handle other value types similarly to the original print_params
            print_params(value, depth, debug);
        }
    }
}

fn print_params(value: &Value, depth: usize, debug: bool) {
    let indent = INDENT.repeat(depth);

    match value {
        Value::Object(map) => {
            for (key, val) in map {
                match val {
                    Value::Object(_) => {
                        println!("{}{}:", indent, style(key).dim());
                        print_params(val, depth + 1, debug);
                    }
                    Value::Array(arr) => {
                        println!("{}{}:", indent, style(key).dim());
                        for item in arr.iter() {
                            println!("{}{}- ", indent, INDENT);
                            print_params(item, depth + 2, debug);
                        }
                    }
                    Value::String(s) => {
                        if !debug && s.len() > get_tool_params_max_length() {
                            println!("{}{}: {}", indent, style(key).dim(), style("...").dim());
                        } else {
                            println!("{}{}: {}", indent, style(key).dim(), style(s).green());
                        }
                    }
                    Value::Number(n) => {
                        println!("{}{}: {}", indent, style(key).dim(), style(n).blue());
                    }
                    Value::Bool(b) => {
                        println!("{}{}: {}", indent, style(key).dim(), style(b).blue());
                    }
                    Value::Null => {
                        println!("{}{}: {}", indent, style(key).dim(), style("null").dim());
                    }
                }
            }
        }
        Value::Array(arr) => {
            for (i, item) in arr.iter().enumerate() {
                println!("{}{}.", indent, i + 1);
                print_params(item, depth + 1, debug);
            }
        }
        Value::String(s) => {
            if !debug && s.len() > get_tool_params_max_length() {
                println!(
                    "{}{}",
                    indent,
                    style(format!("[REDACTED: {} chars]", s.len())).yellow()
                );
            } else {
                println!("{}{}", indent, style(s).green());
            }
        }
        Value::Number(n) => {
            println!("{}{}", indent, style(n).yellow());
        }
        Value::Bool(b) => {
            println!("{}{}", indent, style(b).yellow());
        }
        Value::Null => {
            println!("{}{}", indent, style("null").dim());
        }
    }
}

fn shorten_path(path: &str, debug: bool) -> String {
    // In debug mode, return the full path
    if debug {
        return path.to_string();
    }

    let path = Path::new(path);

    // First try to convert to ~ if it's in home directory
    let home = etcetera::home_dir().ok();
    let path_str = if let Some(home) = home {
        if let Ok(stripped) = path.strip_prefix(home) {
            format!("~/{}", stripped.display())
        } else {
            path.display().to_string()
        }
    } else {
        path.display().to_string()
    };

    // If path is already short enough, return as is
    if path_str.len() <= 60 {
        return path_str;
    }

    let parts: Vec<_> = path_str.split('/').collect();

    // If we have 3 or fewer parts, return as is
    if parts.len() <= 3 {
        return path_str;
    }

    // Keep the first component (empty string before root / or ~) and last two components intact
    let mut shortened = vec![parts[0].to_string()];

    // Shorten middle components to their first letter
    for component in &parts[1..parts.len() - 2] {
        if !component.is_empty() {
            shortened.push(component.chars().next().unwrap_or('?').to_string());
        }
    }

    // Add the last two components
    shortened.push(parts[parts.len() - 2].to_string());
    shortened.push(parts[parts.len() - 1].to_string());

    shortened.join("/")
}

// Session display functions
pub fn display_session_info(
    resume: bool,
    provider: &str,
    model: &str,
    session_file: &Path,
    provider_instance: Option<&Arc<dyn goose::providers::base::Provider>>,
) {
    // Create a modern header with better visual separation
    println!();
    println!(
        "{}",
        style("┌─ Goose Session ─────────────────────────────────────────────────────────────┐")
            .dim()
    );

    let status_icon = if resume {
        "↻"
    } else if session_file.to_str() == Some("/dev/null") || session_file.to_str() == Some("NUL") {
        "⚡"
    } else {
        "▶"
    };

    let status_text = if resume {
        "Resuming session"
    } else if session_file.to_str() == Some("/dev/null") || session_file.to_str() == Some("NUL") {
        "Running without session"
    } else {
        "Starting new session"
    };

    // Box width is 79 chars (77 content + 2 for │ chars)
    // Content area is 77 chars wide
    let content_width = 77;
    let status_line_content = format!("{} {}", status_icon, status_text);
    let status_padding = calculate_padding(&status_line_content, content_width);

    println!(
        "│ {}{}│",
        style(format!("{} {}", status_icon, status_text))
            .green()
            .bold(),
        " ".repeat(status_padding)
    );

    // Check if we have lead/worker mode
    if let Some(provider_inst) = provider_instance {
        if let Some(lead_worker) = provider_inst.as_lead_worker() {
            let (lead_model, worker_model) = lead_worker.get_model_info();
            let provider_line_content = format!(
                "Provider: {} • Lead: {} • Worker: {}",
                provider, lead_model, worker_model
            );
            let provider_padding = calculate_padding(&provider_line_content, content_width);
            println!(
                "│ {} {} {} {}{}│",
                style("Provider:").dim(),
                style(provider).cyan(),
                style("•").dim(),
                style(format!("Lead: {} • Worker: {}", lead_model, worker_model)).cyan(),
                " ".repeat(provider_padding)
            );
        } else {
            let provider_line_content = format!("Provider: {} • {}", provider, model);
            let provider_padding = calculate_padding(&provider_line_content, content_width);
            println!(
                "│ {} {} {} {}{}│",
                style("Provider:").dim(),
                style(provider).cyan(),
                style("•").dim(),
                style(model).cyan(),
                " ".repeat(provider_padding)
            );
        }
    } else {
        // Fallback to original behavior if no provider instance
        let provider_line_content = format!("Provider: {} • {}", provider, model);
        let provider_padding = calculate_padding(&provider_line_content, content_width);
        println!(
            "│ {} {} {} {}{}│",
            style("Provider:").dim(),
            style(provider).cyan(),
            style("•").dim(),
            style(model).cyan(),
            " ".repeat(provider_padding)
        );
    }

    if session_file.to_str() != Some("/dev/null") && session_file.to_str() != Some("NUL") {
        let session_path = session_file.display().to_string();
        let truncated_path = if session_path.len() > 60 {
            format!("...{}", &session_path[session_path.len() - 57..])
        } else {
            session_path.clone()
        };
        let session_line_content = format!("Session: {}", truncated_path);
        let session_padding = calculate_padding(&session_line_content, content_width);
        println!(
            "│ {} {}{}│",
            style("Session:").dim(),
            style(&truncated_path).cyan().dim(),
            " ".repeat(session_padding)
        );
    }

    let working_dir = std::env::current_dir().unwrap().display().to_string();
    let truncated_dir = if working_dir.len() > 60 {
        format!("...{}", &working_dir[working_dir.len() - 57..])
    } else {
        working_dir.clone()
    };
    let directory_line_content = format!("Directory: {}", truncated_dir);
    let directory_padding = calculate_padding(&directory_line_content, content_width);
    println!(
        "│ {} {}{}│",
        style("Directory:").dim(),
        style(&truncated_dir).cyan().dim(),
        " ".repeat(directory_padding)
    );

    println!(
        "{}",
        style("└─────────────────────────────────────────────────────────────────────────────┘")
            .dim()
    );
    println!();
}

pub fn display_greeting() {
    println!(
        "{}",
        style("┌─ Ready to Help ─────────────────────────────────────────────────────────────┐")
            .dim()
    );

    let content_width = 77;

    let line1_content = "🪿 Goose is ready to assist you!";
    let line1_padding = calculate_padding(line1_content, content_width);
    println!(
        "│ {}{}│",
        style(line1_content).bold(),
        " ".repeat(line1_padding)
    );

    let line2_content = "💬 Enter your instructions or ask what I can do";
    let line2_padding = calculate_padding(line2_content, content_width);
    println!(
        "│ {}{}│",
        style(line2_content).dim(),
        " ".repeat(line2_padding)
    );

    let line3_content = "ℹ️ Type /help for available commands";
    let line3_padding = calculate_padding(line3_content, content_width);
    println!(
        "│ {}{}│",
        style(line3_content).dim(),
        " ".repeat(line3_padding)
    );

    println!(
        "{}",
        style("└─────────────────────────────────────────────────────────────────────────────┘")
            .dim()
    );
    println!();
}

/// Display context window usage with both current and session totals
pub fn display_context_usage(total_tokens: usize, context_limit: usize) {
    use console::style;

    // Calculate percentage used
    let percentage = (total_tokens as f64 / context_limit as f64 * 100.0).round() as usize;

    // Create a modern progress bar
    let bar_width = 20;
    let filled_width = ((percentage as f64 / 100.0) * bar_width as f64).round() as usize;
    let empty_width = bar_width - filled_width;

    let filled = "█".repeat(filled_width);
    let empty = "░".repeat(empty_width);

    // Combine bars and apply color
    let bar = format!("{}{}", filled, empty);
    let colored_bar = if percentage < 50 {
        style(bar).green()
    } else if percentage < 85 {
        style(bar).yellow()
    } else {
        style(bar).red()
    };

    // Format numbers with thousands separators
    let formatted_total = format_number(total_tokens);
    let formatted_limit = format_number(context_limit);

    // Print the modern status line
    println!("╭─ Context Usage ─────────────────────────────────────────────────────────────╮");

    let content_width = 77;
    // Calculate the content length without styling for accurate padding
    let context_line_content = format!(
        "{} {:3}% │ {} / {} tokens",
        "█".repeat(bar_width), // Use a consistent character for length calculation
        percentage,
        formatted_total,
        formatted_limit
    );
    let context_padding = calculate_padding(&context_line_content, content_width);

    println!(
        "│ {} {}% │ {} / {} tokens {}│",
        colored_bar,
        style(format!("{:3}", percentage)).bold(),
        style(&formatted_total).cyan(),
        style(&formatted_limit).dim(),
        " ".repeat(context_padding)
    );
    println!("╰─────────────────────────────────────────────────────────────────────────────╯");
}

// Helper function to format numbers with thousands separators
fn format_number(n: usize) -> String {
    let s = n.to_string();
    let mut result = String::new();
    for (i, c) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            result.push(',');
        }
        result.push(c);
    }
    result.chars().rev().collect()
}

// Helper function to calculate display width accounting for Unicode characters
fn display_width(s: &str) -> usize {
    s.chars()
        .map(|c| {
            match c {
                // Emojis and special Unicode chars take 2 display columns
                '🪿' | '🔧' | '💬' | 'ℹ' | '️' | '↻' | '⚡' | '▶' | '🐛' | '🪱' | '🐍' => {
                    2
                }
                // Most other characters take 1 column
                _ => 1,
            }
        })
        .sum()
}

// Helper function to calculate padding accounting for display width
fn calculate_padding(content: &str, target_width: usize) -> usize {
    let display_len = display_width(content);
    target_width.saturating_sub(display_len)
}

pub struct McpSpinners {
    bars: HashMap<String, ProgressBar>,
    log_spinner: Option<ProgressBar>,

    multi_bar: MultiProgress,
}

impl McpSpinners {
    pub fn new() -> Self {
        McpSpinners {
            bars: HashMap::new(),
            log_spinner: None,
            multi_bar: MultiProgress::new(),
        }
    }

    pub fn log(&mut self, message: &str) {
        let spinner = self.log_spinner.get_or_insert_with(|| {
            let bar = self.multi_bar.add(
                ProgressBar::new_spinner()
                    .with_style(
                        ProgressStyle::with_template("{spinner:.green} {msg}")
                            .unwrap()
                            .tick_chars("⠋⠙⠚⠛⠓⠒⠊⠉"),
                    )
                    .with_message(message.to_string()),
            );
            bar.enable_steady_tick(Duration::from_millis(100));
            bar
        });

        spinner.set_message(message.to_string());
    }

    pub fn update(&mut self, token: &str, value: f64, total: Option<f64>, message: Option<&str>) {
        let bar = self.bars.entry(token.to_string()).or_insert_with(|| {
            if let Some(total) = total {
                self.multi_bar.add(
                    ProgressBar::new((total * 100.0) as u64).with_style(
                        ProgressStyle::with_template("[{elapsed}] {bar:40} {pos:>3}/{len:3} {msg}")
                            .unwrap(),
                    ),
                )
            } else {
                self.multi_bar.add(ProgressBar::new_spinner())
            }
        });
        bar.set_position((value * 100.0) as u64);
        if let Some(msg) = message {
            bar.set_message(msg.to_string());
        }
    }

    pub fn hide(&mut self) -> Result<(), Error> {
        self.bars.iter_mut().for_each(|(_, bar)| {
            bar.disable_steady_tick();
        });
        if let Some(spinner) = self.log_spinner.as_mut() {
            spinner.disable_steady_tick();
        }
        self.multi_bar.clear()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    #[test]
    fn test_short_paths_unchanged() {
        assert_eq!(shorten_path("/usr/bin", false), "/usr/bin");
        assert_eq!(shorten_path("/a/b/c", false), "/a/b/c");
        assert_eq!(shorten_path("file.txt", false), "file.txt");
    }

    #[test]
    fn test_debug_mode_returns_full_path() {
        assert_eq!(
            shorten_path("/very/long/path/that/would/normally/be/shortened", true),
            "/very/long/path/that/would/normally/be/shortened"
        );
    }

    #[test]
    fn test_home_directory_conversion() {
        // Save the current home dir
        let original_home = env::var("HOME").ok();

        // Set a test home directory
        env::set_var("HOME", "/Users/testuser");

        assert_eq!(
            shorten_path("/Users/testuser/documents/file.txt", false),
            "~/documents/file.txt"
        );

        // A path that starts similarly to home but isn't in home
        assert_eq!(
            shorten_path("/Users/testuser2/documents/file.txt", false),
            "/Users/testuser2/documents/file.txt"
        );

        // Restore the original home dir
        if let Some(home) = original_home {
            env::set_var("HOME", home);
        } else {
            env::remove_var("HOME");
        }
    }

    #[test]
    fn test_long_path_shortening() {
        assert_eq!(
            shorten_path(
                "/vvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvv/long/path/with/many/components/file.txt",
                false
            ),
            "/v/l/p/w/m/components/file.txt"
        );
    }
}
