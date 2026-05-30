// QPOS Standalone Desktop Print Driver Rust Backend (Tauri v2)

use std::thread;
use std::sync::Mutex;
use std::process::Command;
use std::fs;
use serde::{Serialize, Deserialize};
use serde_json::json;
use tauri::{AppHandle, Manager};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{TrayIcon, TrayIconBuilder, TrayIconEvent, MouseButton};
use base64::Engine;

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

// Flag constant for hiding terminal popups on Windows
const CREATE_NO_WINDOW: u32 = 0x08000000;

// Helper to configure command silent execution
fn configure_silent_command(cmd: &mut Command) {
    #[cfg(target_os = "windows")]
    {
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
}

// Structural print job logs kept in-memory
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PrintLog {
    timestamp: String,
    document_type: String,
    printer_name: String,
    print_engine: String,
    status: String,
    details: String,
}

// Driver configuration structure for local persistence
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct DriverSettings {
    pub default_printer: Option<String>,
    pub auto_reconnect: bool,
}

impl Default for DriverSettings {
    fn default() -> Self {
        DriverSettings {
            default_printer: None,
            auto_reconnect: true,
        }
    }
}

pub struct AppState {
    logs: Mutex<Vec<PrintLog>>,
    settings: Mutex<DriverSettings>,
    settings_path: std::path::PathBuf,
}

// Helper to load settings from file
fn load_settings(path: &std::path::Path) -> DriverSettings {
    if !path.exists() {
        let defaults = DriverSettings::default();
        let _ = save_settings_to_file(path, &defaults);
        return defaults;
    }
    match fs::read_to_string(path) {
        Ok(content) => serde_json::from_str(&content).unwrap_or_else(|_| {
            let defaults = DriverSettings::default();
            let _ = save_settings_to_file(path, &defaults);
            defaults
        }),
        Err(_) => DriverSettings::default(),
    }
}

// Helper to save settings to file
fn save_settings_to_file(path: &std::path::Path, settings: &DriverSettings) -> Result<(), String> {
    let json_str = serde_json::to_string_pretty(settings)
        .map_err(|err| format!("Failed to serialize settings: {}", err))?;
    fs::write(path, json_str)
        .map_err(|err| format!("Failed to write settings file: {}", err))?;
    Ok(())
}

// Command exposed to the dashboard webview to fetch print history logs
#[tauri::command]
fn get_print_logs(state: tauri::State<'_, AppState>) -> Vec<PrintLog> {
    let logs = state.logs.lock().unwrap();
    logs.clone()
}

// Command exposed to clear print logs
#[tauri::command]
fn clear_print_logs(state: tauri::State<'_, AppState>) {
    let mut logs = state.logs.lock().unwrap();
    logs.clear();
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            // Setup app settings persistence path in the OS app config directory
            let config_dir = app.path().app_config_dir().unwrap_or_else(|_| {
                std::env::current_dir().unwrap_or_default()
            });
            let _ = fs::create_dir_all(&config_dir);
            let settings_path = config_dir.join("settings.json");

            // Load persistent settings
            let settings = load_settings(&settings_path);
            let mut verified_settings = settings.clone();
            let mut warning_log = None;

            // Startup Validation: Check if the default printer still exists on system
            if let Some(ref default_prn) = settings.default_printer {
                let installed = get_installed_printers();
                if !installed.contains(default_prn) {
                    // Clear the default printer configuration since it no longer exists
                    verified_settings.default_printer = None;
                    let _ = save_settings_to_file(&settings_path, &verified_settings);

                    warning_log = Some(PrintLog {
                        timestamp: get_current_time_str(),
                        document_type: "System".to_string(),
                        printer_name: default_prn.clone(),
                        print_engine: "Startup Validator".to_string(),
                        status: "Warning".to_string(),
                        details: format!("Default printer '{}' not found on system. Cleared setting.", default_prn),
                    });
                }
            }

            // Manage persistent AppState
            app.manage(AppState {
                logs: Mutex::new(if let Some(log) = warning_log { vec![log] } else { Vec::new() }),
                settings: Mutex::new(verified_settings),
                settings_path,
            });

            // 1. Build and Setup System Tray Icon & Context Menu (Tauri v2)
            let show_item = MenuItem::with_id(app, "show", "Open Diagnostics Dashboard", true, None::<&str>)?;
            let quit_item = MenuItem::with_id(app, "quit", "Exit Print Driver", true, None::<&str>)?;
            let tray_menu = Menu::with_items(app, &[&show_item, &quit_item])?;

            let _tray = TrayIconBuilder::new()
                .menu(&tray_menu)
                .on_menu_event(|app: &tauri::AppHandle, event| {
                    match event.id.as_ref() {
                        "show" => {
                            if let Some(window) = app.get_webview_window("main") {
                                let _ = window.show();
                                let _ = window.set_focus();
                            }
                        }
                        "quit" => {
                            app.exit(0);
                        }
                        _ => {}
                    }
                })
                .on_tray_icon_event(|tray: &TrayIcon, event| {
                    // Restore/Show window on double-clicking or left-clicking the tray icon
                    if let TrayIconEvent::Click { button: MouseButton::Left, .. } = event {
                        let app = tray.app_handle();
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                })
                .icon(app.default_window_icon().unwrap().clone())
                .build(app)?;

            // 2. Launch Local Print Server natively in compiled Rust in a background thread
            let app_handle = app.handle().clone();
            thread::spawn(move || {
                start_native_http_server(app_handle);
            });

            Ok(())
        })
        .on_window_event(|window, event| {
            // 3. Close-to-Tray interception
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![get_print_logs, clear_print_logs])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

// Natively compiled lightweight HTTP print server using tiny_http crate
fn start_native_http_server(app: AppHandle) {
    let server = match tiny_http::Server::http("127.0.0.1:9000") {
        Ok(srv) => srv,
        Err(err) => {
            eprintln!("Failed to bind local HTTP print server on port 9000: {}", err);
            return;
        }
    };

    println!("======================================================");
    // Logging locally for developer confirmation
    println!("🚀 QPOS RUST-NATIVE DESKTOP DRIVER LISTENING ON PORT 9000!");
    println!("======================================================");

    for mut request in server.incoming_requests() {
        let url = request.url().to_string();
        let method = request.method().to_string();

        // CORS Preflight headers - Intercepts OPTIONS for all endpoints
        if method == "OPTIONS" {
            let response = tiny_http::Response::empty(204)
                .with_header(tiny_http::Header::from_bytes(&b"Access-Control-Allow-Origin"[..], &b"*"[..]).unwrap())
                .with_header(tiny_http::Header::from_bytes(&b"Access-Control-Allow-Methods"[..], &b"GET, POST, OPTIONS"[..]).unwrap())
                .with_header(tiny_http::Header::from_bytes(&b"Access-Control-Allow-Headers"[..], &b"Content-Type"[..]).unwrap());
            let _ = request.respond(response);
            continue;
        }

        // GET /printers
        if url == "/printers" && method == "GET" {
            let printers = get_installed_printers();
            let json_res = json!({ "success": true, "printers": printers });
            let json_str = serde_json::to_string(&json_res).unwrap_or_else(|_| "{}".to_string());

            let response = tiny_http::Response::from_string(json_str)
                .with_status_code(200)
                .with_header(tiny_http::Header::from_bytes(&b"Access-Control-Allow-Origin"[..], &b"*"[..]).unwrap())
                .with_header(tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap());
            let _ = request.respond(response);
            continue;
        }

        // GET /settings
        if url == "/settings" && method == "GET" {
            if let Some(state) = app.try_state::<AppState>() {
                let settings = state.settings.lock().unwrap();
                let json_res = serde_json::to_value(&*settings).unwrap_or(json!({}));

                let response = tiny_http::Response::from_string(json_res.to_string())
                    .with_status_code(200)
                    .with_header(tiny_http::Header::from_bytes(&b"Access-Control-Allow-Origin"[..], &b"*"[..]).unwrap())
                    .with_header(tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap());
                let _ = request.respond(response);
            }
            continue;
        }

        // POST /settings
        if url == "/settings" && method == "POST" {
            let mut content = String::new();
            let _ = request.as_reader().read_to_string(&mut content);

            match serde_json::from_str::<DriverSettings>(&content) {
                Ok(new_settings) => {
                    if let Some(state) = app.try_state::<AppState>() {
                        let mut settings = state.settings.lock().unwrap();
                        let old_printer = settings.default_printer.clone();
                        let new_printer = new_settings.default_printer.clone();

                        *settings = new_settings.clone();
                        let _ = save_settings_to_file(&state.settings_path, &*settings);

                        // Log warning/success on change
                        if old_printer != new_printer {
                            let log_entry = PrintLog {
                                timestamp: get_current_time_str(),
                                document_type: "System".to_string(),
                                printer_name: new_printer.clone().unwrap_or_else(|| "None".to_string()),
                                print_engine: "Settings Manager".to_string(),
                                status: "Success".to_string(),
                                details: format!(
                                    "Default printer changed from '{}' to '{}'",
                                    old_printer.unwrap_or_else(|| "None".to_string()),
                                    new_printer.unwrap_or_else(|| "None".to_string())
                                ),
                            };
                            state.logs.lock().unwrap().push(log_entry);
                        }

                        let api_res = json!({ "success": true });
                        let response = tiny_http::Response::from_string(api_res.to_string())
                            .with_status_code(200)
                            .with_header(tiny_http::Header::from_bytes(&b"Access-Control-Allow-Origin"[..], &b"*"[..]).unwrap())
                            .with_header(tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap());
                        let _ = request.respond(response);
                    }
                }
                Err(err) => {
                    let api_res = json!({ "success": false, "error": "Invalid settings payload", "details": err.to_string() });
                    let response = tiny_http::Response::from_string(api_res.to_string())
                        .with_status_code(400)
                        .with_header(tiny_http::Header::from_bytes(&b"Access-Control-Allow-Origin"[..], &b"*"[..]).unwrap())
                        .with_header(tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap());
                    let _ = request.respond(response);
                }
            }
            continue;
        }

        // GET /settings/default-printer
        if url == "/settings/default-printer" && method == "GET" {
            if let Some(state) = app.try_state::<AppState>() {
                let settings = state.settings.lock().unwrap();
                let api_res = json!({ "printerName": settings.default_printer.clone().unwrap_or_default() });

                let response = tiny_http::Response::from_string(api_res.to_string())
                    .with_status_code(200)
                    .with_header(tiny_http::Header::from_bytes(&b"Access-Control-Allow-Origin"[..], &b"*"[..]).unwrap())
                    .with_header(tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap());
                let _ = request.respond(response);
            }
            continue;
        }

        // POST /settings/default-printer
        if url == "/settings/default-printer" && method == "POST" {
            let mut content = String::new();
            let _ = request.as_reader().read_to_string(&mut content);

            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct DefaultPrnPayload {
                printer_name: String,
            }

            match serde_json::from_str::<DefaultPrnPayload>(&content) {
                Ok(payload) => {
                    if let Some(state) = app.try_state::<AppState>() {
                        let mut settings = state.settings.lock().unwrap();
                        let old_printer = settings.default_printer.clone();
                        let new_printer = if payload.printer_name.is_empty() { None } else { Some(payload.printer_name.clone()) };

                        settings.default_printer = new_printer.clone();
                        let _ = save_settings_to_file(&state.settings_path, &*settings);

                        // Log change
                        if old_printer != new_printer {
                            let log_entry = PrintLog {
                                timestamp: get_current_time_str(),
                                document_type: "System".to_string(),
                                printer_name: new_printer.clone().unwrap_or_else(|| "None".to_string()),
                                print_engine: "Settings Manager".to_string(),
                                status: "Success".to_string(),
                                details: format!(
                                    "Default printer changed from '{}' to '{}'",
                                    old_printer.unwrap_or_else(|| "None".to_string()),
                                    new_printer.unwrap_or_else(|| "None".to_string())
                                ),
                            };
                            state.logs.lock().unwrap().push(log_entry);
                        }

                        let api_res = json!({ "success": true });
                        let response = tiny_http::Response::from_string(api_res.to_string())
                            .with_status_code(200)
                            .with_header(tiny_http::Header::from_bytes(&b"Access-Control-Allow-Origin"[..], &b"*"[..]).unwrap())
                            .with_header(tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap());
                        let _ = request.respond(response);
                    }
                }
                Err(err) => {
                    let api_res = json!({ "success": false, "error": "Invalid printerName payload", "details": err.to_string() });
                    let response = tiny_http::Response::from_string(api_res.to_string())
                        .with_status_code(400)
                        .with_header(tiny_http::Header::from_bytes(&b"Access-Control-Allow-Origin"[..], &b"*"[..]).unwrap())
                        .with_header(tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap());
                    let _ = request.respond(response);
                }
            }
            continue;
        }

        // POST /print
        if url == "/print" && method == "POST" {
            let mut content = String::new();
            let _ = request.as_reader().read_to_string(&mut content);

            #[derive(Deserialize)]
            #[allow(non_snake_case)]
            struct PrintPayload {
                printerName: Option<String>,
                fileContentBase64: String,
                fileExtension: Option<String>,
                printHandler: Option<String>,
            }

            match serde_json::from_str::<PrintPayload>(&content) {
                Ok(payload) => {
                    let file_ext = payload.fileExtension.unwrap_or_else(|| ".bin".to_string());
                    let print_handler = payload.printHandler.unwrap_or_else(|| "word".to_string());

                    // Resolve printer name with backward compatibility support
                    let mut printer_to_use = payload.printerName.unwrap_or_default();
                    if printer_to_use.is_empty() {
                        if let Some(state) = app.try_state::<AppState>() {
                            let settings = state.settings.lock().unwrap();
                            if let Some(ref def_prn) = settings.default_printer {
                                printer_to_use = def_prn.clone();
                            }
                        }
                    }

                    // Return detailed error if no printer target is resolved
                    if printer_to_use.is_empty() {
                        let api_res = json!({
                            "success": false,
                            "error": "No printer specified and no default printer configured."
                        });
                        let json_str = serde_json::to_string(&api_res).unwrap_or_else(|_| "{}".to_string());
                        let response = tiny_http::Response::from_string(json_str)
                            .with_status_code(200) // standard 200 containing error message for client parsing compatibility
                            .with_header(tiny_http::Header::from_bytes(&b"Access-Control-Allow-Origin"[..], &b"*"[..]).unwrap())
                            .with_header(tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap());
                        let _ = request.respond(response);
                        continue;
                    }

                    let res = execute_print_job(
                        &printer_to_use,
                        &payload.fileContentBase64,
                        &file_ext,
                        &print_handler,
                    );

                    let status_str = if res.is_ok() { "Success".to_string() } else { "Failed".to_string() };
                    let details_str = match &res {
                        Ok(msg) => msg.clone(),
                        Err(err) => err.clone(),
                    };

                    // Add record log entry to state
                    let log_entry = PrintLog {
                        timestamp: get_current_time_str(),
                        document_type: if file_ext == ".bin" { "POS Slip".to_string() } else { "Office Template".to_string() },
                        printer_name: printer_to_use.clone(),
                        print_engine: if file_ext == ".bin" { "Thermal ESC/POS".to_string() } else if print_handler == "wordpad" { "Windows WordPad".to_string() } else { "Microsoft Word".to_string() },
                        status: status_str,
                        details: details_str,
                    };

                    if let Some(state) = app.try_state::<AppState>() {
                        state.logs.lock().unwrap().push(log_entry);
                    }

                    let api_res = match res {
                        Ok(msg) => json!({ "success": true, "message": msg }),
                        Err(err) => json!({ "success": false, "error": err }),
                    };

                    let json_str = serde_json::to_string(&api_res).unwrap_or_else(|_| "{}".to_string());
                    let response = tiny_http::Response::from_string(json_str)
                        .with_status_code(200)
                        .with_header(tiny_http::Header::from_bytes(&b"Access-Control-Allow-Origin"[..], &b"*"[..]).unwrap())
                        .with_header(tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap());
                    let _ = request.respond(response);
                }
                Err(err) => {
                    let api_res = json!({ "success": false, "error": "Invalid JSON print request", "details": err.to_string() });
                    let json_str = serde_json::to_string(&api_res).unwrap_or_else(|_| "{}".to_string());
                    let response = tiny_http::Response::from_string(json_str)
                        .with_status_code(400)
                        .with_header(tiny_http::Header::from_bytes(&b"Access-Control-Allow-Origin"[..], &b"*"[..]).unwrap())
                        .with_header(tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap());
                    let _ = request.respond(response);
                }
            }
            continue;
        }

        // Return 404 for other endpoints
        let response = tiny_http::Response::empty(404)
            .with_header(tiny_http::Header::from_bytes(&b"Access-Control-Allow-Origin"[..], &b"*"[..]).unwrap());
        let _ = request.respond(response);
    }
}

// Discovers local printers using lightweight PowerShell cmd (Silent running)
fn get_installed_printers() -> Vec<String> {
    let mut cmd = Command::new("powershell");
    cmd.args(&[
        "-NoProfile",
        "-Command",
        "Get-CimInstance Win32_Printer | Select-Object -ExpandProperty Name"
    ]);
    configure_silent_command(&mut cmd);
    
    let output = cmd.output();

    match output {
        Ok(out) => {
            let text = String::from_utf8_lossy(&out.stdout);
            text.lines()
                .map(|line| line.trim().to_string())
                .filter(|line| !line.is_empty())
                .collect()
        }
        Err(_) => vec![],
    }
}

// Performs silent spool printing using WScript/WordPad fallback default printer switching
fn execute_print_job(
    printer_name: &str,
    base64_content: &str,
    file_ext: &str,
    print_handler: &str,
) -> Result<String, String> {
    // 1. Decode base64 buffer payload
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(base64_content)
        .map_err(|err| format!("Base64 decode failed: {}", err))?;

    // 2. Setup temporary file paths in OS temp directory
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis();
    
    let temp_dir = std::env::temp_dir();
    let temp_print_file = temp_dir.join(format!("qpos_tauri_job_{}{}", timestamp, file_ext));
    let temp_script_file = temp_dir.join(format!("qpos_tauri_script_{}.ps1", timestamp));

    // 3. Write print payload bytes to file
    fs::write(&temp_print_file, bytes)
        .map_err(|err| format!("Failed to create temporary print file: {}", err))?;

    // 4. Generate clean, robust PowerShell script contents
    let ps_script_content = format!(
        r#"
$printerName = "{}"
$filePath = "{}"
$printHandler = "{}"
$extension = [System.IO.Path]::GetExtension($filePath).ToLower()

if ($extension -eq ".bin") {{
    $code = @"
using System;
using System.Runtime.InteropServices;

public class RawPrinterHelper {{
    [DllImport("winspool.Drv", EntryPoint = "OpenPrinterA", SetLastError = true, CharSet = CharSet.Ansi)]
    public static extern bool OpenPrinter(string printerName, out IntPtr phPrinter, IntPtr pDefault);

    [DllImport("winspool.Drv", EntryPoint = "StartDocPrinterA", SetLastError = true, CharSet = CharSet.Ansi)]
    public static extern bool StartDocPrinter(IntPtr hPrinter, int level, [In, MarshalAs(UnmanagedType.LPStruct)] DOCINFOA pDocInfo);

    [DllImport("winspool.Drv", EntryPoint = "WritePrinter", SetLastError = true)]
    public static extern bool WritePrinter(IntPtr hPrinter, IntPtr pBytes, int dwCount, out int dwWritten);

    [DllImport("winspool.Drv", EntryPoint = "EndDocPrinter", SetLastError = true)]
    public static extern bool EndDocPrinter(IntPtr hPrinter);

    [DllImport("winspool.Drv", EntryPoint = "ClosePrinter", SetLastError = true)]
    public static extern bool ClosePrinter(IntPtr hPrinter);

    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Ansi)]
    public class DOCINFOA {{
        [MarshalAs(UnmanagedType.LPStr)] public string pDocName;
        [MarshalAs(UnmanagedType.LPStr)] public string pOutputFile;
        [MarshalAs(UnmanagedType.LPStr)] public string pDatatype;
    }}

    public static bool SendBytesToPrinter(string printerName, byte[] bytes) {{
        IntPtr hPrinter;
        if (!OpenPrinter(printerName, out hPrinter, IntPtr.Zero)) return false;
        
        DOCINFOA di = new DOCINFOA {{ pDocName = "POS Receipt Thermal Print", pDatatype = "RAW" }};
        if (!StartDocPrinter(hPrinter, 1, di)) {{
            ClosePrinter(hPrinter);
            return false;
        }}
        
        IntPtr pBytes = Marshal.AllocHGlobal(bytes.Length);
        Marshal.Copy(bytes, 0, pBytes, bytes.Length);
        int written;
        bool success = WritePrinter(hPrinter, pBytes, bytes.Length, out written);
        Marshal.FreeHGlobal(pBytes);
        
        EndDocPrinter(hPrinter);
        ClosePrinter(hPrinter);
        return success;
    }}
}}
"@
    if (-not ([System.Management.Automation.PSTypeName]'RawPrinterHelper').Type) {{
        Add-Type -TypeDefinition $code
    }}
    $bytes = [System.IO.File]::ReadAllBytes($filePath)
    $success = [RawPrinterHelper]::SendBytesToPrinter($printerName, $bytes)
    if ($success) {{
        Write-Output "SUCCESS: Thermal slip printed."
    }} else {{
        Write-Error "FAILED: Thermal Spooling failure."
        exit 1
    }}
}} else {{
    $originalPrinter = (Get-CimInstance -ClassName CIM_Printer | Where-Object {{ $_.Default -eq $true }}).Name
    $changedDefault = $false
    try {{
        $targetPrn = Get-CimInstance -ClassName CIM_Printer | Where-Object {{ $_.Name -eq $printerName }}
        if (-not $targetPrn) {{
            Write-Error "FAILED: Printer '$printerName' not found."
            exit 1
        }}
        if ($printerName -ne $originalPrinter) {{
            (New-Object -ComObject WScript.Network).SetDefaultPrinter($printerName)
            $changedDefault = $true
            Start-Sleep -Seconds 1
        }}
        if ($printHandler -eq "wordpad") {{
            $proc = Start-Process -FilePath "wordpad.exe" -ArgumentList "/p `"$filePath`"" -PassThru
        }} else {{
            $proc = Start-Process -FilePath $filePath -Verb Print -PassThru
        }}
        Start-Sleep -Seconds 5
        if ($proc -and -not $proc.HasExited) {{
            $proc.CloseMainWindow()
            Start-Sleep -Seconds 1
            if (-not $proc.HasExited) {{ $proc.Kill() }}
        }}
        Write-Output "SUCCESS: Document printed."
    }} finally {{
        if ($changedDefault -and $originalPrinter) {{
            (New-Object -ComObject WScript.Network).SetDefaultPrinter($originalPrinter)
        }}
    }}
}}
"#,
        printer_name.replace("\"", "`\""),
        temp_print_file.to_string_lossy().replace("\\", "\\\\"),
        print_handler.replace("\"", "`\"")
    );

    // 5. Write PowerShell script contents to file
    fs::write(&temp_script_file, ps_script_content)
        .map_err(|err| format!("Failed to create temporary powershell script: {}", err))?;

    // 6. Execute powershell script (Silently)
    let mut cmd = Command::new("powershell");
    cmd.args(&[
        "-NoProfile",
        "-ExecutionPolicy",
        "Bypass",
        "-File",
        &temp_script_file.to_string_lossy()
    ]);
    configure_silent_command(&mut cmd);

    let output = cmd.output()
        .map_err(|err| format!("Failed to launch powershell execution: {}", err))?;

    // 7. Cleanup temp files asynchronously
    let _ = fs::remove_file(&temp_print_file);
    let _ = fs::remove_file(&temp_script_file);

    // 8. Capture console feedback
    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Ok(stdout)
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(format!("Spooler reported error: {}", stderr))
    }
}

// Native command line helper to fetch local time on Windows systems (Silently)
fn get_current_time_str() -> String {
    let mut cmd_date = Command::new("cmd");
    cmd_date.args(&["/c", "date /t"]);
    configure_silent_command(&mut cmd_date);
    let date_output = cmd_date.output();

    let mut cmd_time = Command::new("cmd");
    cmd_time.args(&["/c", "time /t"]);
    configure_silent_command(&mut cmd_time);
    let time_output = cmd_time.output();
    
    let date_str = match date_output {
        Ok(out) => String::from_utf8_lossy(&out.stdout).trim().to_string(),
        Err(_) => "".to_string()
    };
    let time_str = match time_output {
        Ok(out) => String::from_utf8_lossy(&out.stdout).trim().to_string(),
        Err(_) => "".to_string()
    };
    
    format!("{} {}", date_str, time_str).trim().to_string()
}
