use pinyin::ToPinyin;
use serde::{Deserialize, Serialize};
use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};
use std::{
    ffi::OsStr,
    fs,
    io::{Read, Write},
    net::{Shutdown, TcpListener, TcpStream, ToSocketAddrs},
    path::{Path, PathBuf},
    process::Command,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex, OnceLock,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use tauri::Emitter;
use zip::{write::SimpleFileOptions, CompressionMethod, ZipArchive, ZipWriter};

#[cfg(windows)]
use std::os::windows::{ffi::OsStrExt, process::CommandExt};

const LIBRARY_DIR: &str = "AppManagerLibrary";
const APPS_DIR: &str = "Apps";
const CONFIG_DIR: &str = "config";
const DATA_FILE: &str = "app-data.json";
const REVIEW_DIR: &str = "未审核软件";
const REVIEW_FOLDER: &str = "review-pending";
const PACKAGE_CACHE_DIR: &str = "package-cache";
const APP_ICON_FILE_STEM: &str = ".appmanager-icon";
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;
const ZIP_BUFFER_SIZE: usize = 64 * 1024;
const SERVER_CLIENT_ONLINE_SECONDS: u64 = 30;
const SERVER_CLIENT_KEEP_SECONDS: u64 = 5 * 60;
const CLIENT_STATUS_CONNECT_TIMEOUT_MS: u64 = 1200;
const CLIENT_STATUS_IO_TIMEOUT_MS: u64 = 1500;
const ENABLE_DEBUG_LOGS: bool = false;
static SERVER_RUNTIME: OnceLock<Mutex<Option<ServerRuntime>>> = OnceLock::new();
static SERVER_CLIENTS: OnceLock<Mutex<HashMap<String, ServerClientInfo>>> = OnceLock::new();
static TRANSFER_PROGRESS: OnceLock<Mutex<HashMap<String, TransferProgress>>> = OnceLock::new();
static TRANSFER_SPEED_SAMPLES: OnceLock<Mutex<HashMap<String, TransferSpeedSample>>> =
    OnceLock::new();
static TEMP_FILE_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Category {
    id: String,
    name: String,
    path: String,
    created_at: u64,
    updated_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManagedApp {
    id: String,
    name: String,
    category_id: String,
    category_name: String,
    folder_path: String,
    executable_path: Option<String>,
    #[serde(default)]
    executable_candidates: Vec<String>,
    #[serde(default)]
    icon_data_url: Option<String>,
    favorite: bool,
    note: String,
    launch_count: u64,
    last_launched_at: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Settings {
    #[serde(default = "default_theme")]
    theme: String,
    #[serde(default = "default_grid_density")]
    grid_density: String,
    #[serde(default = "default_startup_view")]
    startup_view: String,
    #[serde(default = "default_run_mode")]
    run_mode: String,
    #[serde(default)]
    autostart_enabled: bool,
    #[serde(default)]
    server: ServerConfig,
    #[serde(default)]
    client: ClientConfig,
    #[serde(default)]
    favorite_order: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ServerConfig {
    #[serde(default = "default_server_host")]
    host: String,
    #[serde(default = "default_server_port")]
    port: u16,
    #[serde(default = "default_server_username")]
    username: String,
    #[serde(default)]
    password: String,
    #[serde(default)]
    password_hash: String,
    #[serde(default = "default_true")]
    allow_downloads: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClientConfig {
    #[serde(default = "default_client_host")]
    host: String,
    #[serde(default = "default_server_port")]
    port: u16,
    #[serde(default = "default_server_username")]
    username: String,
    #[serde(default)]
    password: String,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: default_server_host(),
            port: default_server_port(),
            username: default_server_username(),
            password: String::new(),
            password_hash: String::new(),
            allow_downloads: true,
        }
    }
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            host: default_client_host(),
            port: default_server_port(),
            username: default_server_username(),
            password: String::new(),
        }
    }
}

#[derive(Debug)]
struct ServerRuntime {
    stop: Arc<AtomicBool>,
    host: String,
    port: u16,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ServerClientInfo {
    address: String,
    username: String,
    last_path: String,
    last_seen_at: u64,
    online: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: "light".to_string(),
            grid_density: "comfortable".to_string(),
            startup_view: "favorites".to_string(),
            run_mode: "local".to_string(),
            autostart_enabled: false,
            server: ServerConfig::default(),
            client: ClientConfig::default(),
            favorite_order: Vec::new(),
        }
    }
}

fn default_theme() -> String {
    "light".to_string()
}

fn default_grid_density() -> String {
    "comfortable".to_string()
}

fn default_startup_view() -> String {
    "favorites".to_string()
}

fn default_run_mode() -> String {
    "local".to_string()
}

fn default_server_host() -> String {
    "0.0.0.0".to_string()
}

fn default_server_port() -> u16 {
    8765
}

fn default_server_username() -> String {
    "admin".to_string()
}

fn default_client_host() -> String {
    "127.0.0.1".to_string()
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AppData {
    library_path: String,
    categories: Vec<Category>,
    apps: Vec<ManagedApp>,
    settings: Settings,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LibraryState {
    library_path: String,
    apps_path: String,
    data: AppData,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ScanIssue {
    folder_path: String,
    reason: String,
    candidates: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ScanResult {
    added: usize,
    updated: usize,
    issues: Vec<ScanIssue>,
    data: AppData,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct LaunchResult {
    app_id: String,
    launch_count: u64,
    last_launched_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TransferProgress {
    app_id: String,
    app_name: String,
    direction: String,
    transferred: u64,
    total: u64,
    speed: u64,
    percent: f64,
    status: String,
}

#[derive(Debug, Clone)]
struct TransferSpeedSample {
    transferred: u64,
    timestamp_ms: u128,
    speed: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ReviewItem {
    id: String,
    name: String,
    file_name: String,
    category_name: String,
    size: u64,
    uploaded_at: u64,
    path: String,
    #[serde(skip_serializing)]
    extracted_paths: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateAppInfoRequest {
    app_id: String,
    name: String,
    note: String,
    icon_path: Option<String>,
    #[serde(default)]
    icon_data_url: Option<String>,
    executable_path: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateSettingsRequest {
    run_mode: String,
    theme: String,
    grid_density: String,
    autostart_enabled: bool,
    server_host: String,
    server_port: u16,
    server_username: String,
    server_password: String,
    server_allow_downloads: bool,
    client_host: String,
    client_port: u16,
    client_username: String,
    client_password: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ServerStatus {
    running: bool,
    host: String,
    port: u16,
    clients: Vec<ServerClientInfo>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PackageCacheInfo {
    path: String,
    file_count: u64,
    total_size: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ClientConnectionStatus {
    configured: bool,
    online: bool,
    host: String,
    port: u16,
    username: String,
    message: String,
    server_name: Option<String>,
    server_mode: Option<String>,
    allow_downloads: Option<bool>,
    checked_at: u64,
}

#[derive(Debug)]
struct HttpResponse {
    status: u16,
    body: Vec<u8>,
}

#[derive(Debug)]
struct HttpFileResponse {
    status: u16,
    headers: Vec<(String, String)>,
    body_path: PathBuf,
    transferred: u64,
    total: u64,
}

#[derive(Clone, Copy)]
struct ZipProgress<'a> {
    app_handle: Option<&'a tauri::AppHandle>,
    direction: &'a str,
    app_id: &'a str,
    app_name: &'a str,
    status: &'a str,
}

#[tauri::command]
fn init_library(app_handle: tauri::AppHandle) -> Result<LibraryState, String> {
    let library_path = library_root()?;
    let apps_path = library_path.join(APPS_DIR);
    let config_path = library_path.join(CONFIG_DIR);

    fs::create_dir_all(&apps_path).map_err(error_message)?;
    fs::create_dir_all(&config_path).map_err(error_message)?;

    let data = load_or_create_data(&library_path)?;
    if sync_server(&data.settings, &app_handle).is_err() {
        stop_server();
    }

    Ok(LibraryState {
        library_path: path_to_string(&library_path),
        apps_path: path_to_string(&apps_path),
        data,
    })
}

#[tauri::command]
fn create_category(name: String) -> Result<AppData, String> {
    let clean_name = validate_category_name(&name)?;
    let library_path = library_root()?;
    let mut data = load_or_create_data(&library_path)?;

    if data.categories.iter().any(|item| item.name == clean_name) {
        return Err("分类名称已存在".to_string());
    }

    let now = now();
    let category_path = category_storage_path(&library_path, &clean_name);
    fs::create_dir_all(&category_path).map_err(error_message)?;

    data.categories.push(Category {
        id: make_id(&clean_name),
        name: clean_name,
        path: path_to_string(&category_path),
        created_at: now,
        updated_at: now,
    });

    save_data(&library_path, &data)?;
    Ok(data)
}

#[tauri::command]
fn delete_category(category_id: String, delete_files: bool) -> Result<AppData, String> {
    let library_path = library_root()?;
    let mut data = load_or_create_data(&library_path)?;
    let category = data
        .categories
        .iter()
        .find(|item| item.id == category_id)
        .cloned()
        .ok_or_else(|| "分类不存在".to_string())?;

    if delete_files {
        let category_path = PathBuf::from(&category.path);
        ensure_inside_apps_dir(&library_path, &category_path)?;
        if category_path.exists() {
            fs::remove_dir_all(&category_path).map_err(error_message)?;
        }
    }

    data.categories.retain(|item| item.id != category_id);
    data.apps.retain(|item| item.category_id != category_id);

    save_data(&library_path, &data)?;
    Ok(data)
}

#[tauri::command]
fn scan_category(category_id: String) -> Result<ScanResult, String> {
    let library_path = library_root()?;
    let mut data = load_or_create_data(&library_path)?;
    let category = data
        .categories
        .iter()
        .find(|item| item.id == category_id)
        .cloned()
        .ok_or_else(|| "分类不存在".to_string())?;

    let result = scan_categories(&library_path, &mut data, &[category])?;
    save_data(&library_path, &result.data)?;
    Ok(result)
}

#[tauri::command]
fn scan_all() -> Result<ScanResult, String> {
    let library_path = library_root()?;
    let mut data = load_or_create_data(&library_path)?;
    let categories = data.categories.clone();

    let result = scan_categories(&library_path, &mut data, &categories)?;
    save_data(&library_path, &result.data)?;
    Ok(result)
}

#[tauri::command]
fn toggle_favorite(app_id: String) -> Result<AppData, String> {
    let library_path = library_root()?;
    let mut data = load_or_create_data(&library_path)?;
    let app = data
        .apps
        .iter_mut()
        .find(|item| item.id == app_id)
        .ok_or_else(|| "软件不存在".to_string())?;

    app.favorite = !app.favorite;
    save_data(&library_path, &data)?;
    Ok(data)
}

#[tauri::command]
fn delete_app(app_id: String, delete_files: bool) -> Result<AppData, String> {
    let library_path = library_root()?;
    let mut data = load_or_create_data(&library_path)?;
    let app = data
        .apps
        .iter()
        .find(|item| item.id == app_id)
        .cloned()
        .ok_or_else(|| "软件不存在".to_string())?;

    if delete_files {
        delete_app_files(&library_path, &app)?;
    }

    data.apps.retain(|item| item.id != app_id);
    save_data(&library_path, &data)?;
    Ok(data)
}

#[tauri::command]
fn move_app_to_category(app_id: String, category_id: String) -> Result<AppData, String> {
    let library_path = library_root()?;
    let mut data = load_or_create_data(&library_path)?;
    let target_category = data
        .categories
        .iter()
        .find(|item| item.id == category_id)
        .cloned()
        .ok_or_else(|| "目标分类不存在".to_string())?;
    let app = data
        .apps
        .iter_mut()
        .find(|item| item.id == app_id)
        .ok_or_else(|| "软件不存在".to_string())?;

    if app.category_id == target_category.id {
        return Ok(data);
    }

    let target_category_path = PathBuf::from(normalize_incoming_path(&target_category.path));
    ensure_inside_apps_dir(&library_path, &target_category_path)?;
    fs::create_dir_all(&target_category_path).map_err(error_message)?;

    let old_folder_path = PathBuf::from(normalize_incoming_path(&app.folder_path));
    let old_executable_path = app
        .executable_path
        .as_ref()
        .map(|path| PathBuf::from(normalize_incoming_path(path)));
    ensure_inside_apps_dir(&library_path, &old_folder_path)?;

    if let Some(old_executable_path) = old_executable_path.as_ref() {
        ensure_inside_apps_dir(&library_path, old_executable_path)?;
    }

    let (new_folder_path, new_executable_path) = move_app_files(
        &old_folder_path,
        old_executable_path.as_deref(),
        &target_category_path,
    )?;

    app.category_id = target_category.id;
    app.category_name = target_category.name;
    app.folder_path = path_to_string(&new_folder_path);
    app.executable_path = new_executable_path
        .as_ref()
        .map(|path| path_to_string(path));

    save_data(&library_path, &data)?;
    Ok(data)
}

#[tauri::command]
fn update_app_info(request: UpdateAppInfoRequest) -> Result<AppData, String> {
    let clean_name = request.name.trim();
    if clean_name.is_empty() {
        return Err("软件名称不能为空".to_string());
    }

    let library_path = library_root()?;
    let mut data = load_or_create_data(&library_path)?;
    let app = data
        .apps
        .iter_mut()
        .find(|item| item.id == request.app_id)
        .ok_or_else(|| "软件不存在".to_string())?;

    app.name = clean_name.to_string();
    app.note = request.note.trim().to_string();

    if let Some(executable_path) = request.executable_path {
        let clean_executable_path = executable_path.trim();
        if clean_executable_path.is_empty() {
            app.executable_path = None;
        } else {
            let executable = PathBuf::from(normalize_incoming_path(clean_executable_path));
            ensure_inside_apps_dir(&library_path, &executable)?;
            if !executable.exists() {
                return Err("启动程序不存在".to_string());
            }
            if !is_executable_file(&executable) {
                return Err("启动程序必须是 .exe 文件".to_string());
            }
            let executable_path = path_to_string(&executable);
            app.executable_path = Some(executable_path.clone());
            if !app
                .executable_candidates
                .iter()
                .any(|candidate| candidate == &executable_path)
            {
                app.executable_candidates.push(executable_path);
                app.executable_candidates.sort();
                app.executable_candidates.dedup();
            }
        }
    }

    if let Some(icon_path) = request.icon_path {
        let clean_icon_path = icon_path.trim();
        if !clean_icon_path.is_empty() {
            app.icon_data_url = Some(read_image_as_data_url(clean_icon_path)?);
        }
    }

    if let Some(icon_data_url) = request.icon_data_url {
        app.icon_data_url = Some(validate_image_data_url(&icon_data_url)?);
    }

    save_data(&library_path, &data)?;
    Ok(data)
}

#[tauri::command]
fn update_settings(
    app_handle: tauri::AppHandle,
    request: UpdateSettingsRequest,
) -> Result<AppData, String> {
    if !["local", "server", "client"].contains(&request.run_mode.as_str()) {
        return Err("运行模式无效".to_string());
    }

    if !["comfortable", "compact"].contains(&request.grid_density.as_str()) {
        return Err("网格密度无效".to_string());
    }

    if !["light", "dark", "green"].contains(&request.theme.as_str()) {
        return Err("主题无效".to_string());
    }

    set_windows_autostart(request.autostart_enabled)?;

    let library_path = library_root()?;
    let mut data = load_or_create_data(&library_path)?;
    data.settings.run_mode = request.run_mode;
    data.settings.theme = request.theme;
    data.settings.grid_density = request.grid_density;
    data.settings.autostart_enabled = request.autostart_enabled;
    data.settings.server.host = request.server_host.trim().to_string();
    data.settings.server.port = request.server_port;
    data.settings.server.username = request.server_username.trim().to_string();
    data.settings.server.allow_downloads = request.server_allow_downloads;
    data.settings.client.host = request.client_host.trim().to_string();
    data.settings.client.port = request.client_port;
    data.settings.client.username = request.client_username.trim().to_string();

    let server_password = request.server_password.trim();
    data.settings.server.password = server_password.to_string();
    data.settings.server.password_hash = if server_password.is_empty() {
        String::new()
    } else {
        make_id(server_password)
    };
    data.settings.client.password = request.client_password.trim().to_string();

    save_data(&library_path, &data)?;
    sync_server(&data.settings, &app_handle)?;
    Ok(data)
}

#[tauri::command]
fn get_server_status() -> Result<ServerStatus, String> {
    Ok(current_server_status())
}

#[tauri::command]
fn get_package_cache_info() -> Result<PackageCacheInfo, String> {
    let library_path = library_root()?;
    package_cache_info(&library_path)
}

#[tauri::command]
fn get_client_connection_status() -> Result<ClientConnectionStatus, String> {
    let library_path = library_root()?;
    let data = load_or_create_data(&library_path)?;
    if data.settings.run_mode != "client" {
        return Ok(inactive_client_connection_status(&data.settings));
    }
    Ok(client_connection_status(&data.settings.client))
}

#[tauri::command]
fn update_favorite_order(app_ids: Vec<String>) -> Result<AppData, String> {
    let library_path = library_root()?;
    let mut data = load_or_create_data(&library_path)?;
    let favorites = data
        .apps
        .iter()
        .filter(|app| app.favorite)
        .map(|app| app.id.clone())
        .collect::<HashSet<_>>();
    let mut ordered = Vec::new();
    for app_id in app_ids {
        if favorites.contains(&app_id) && !ordered.iter().any(|item| item == &app_id) {
            ordered.push(app_id);
        }
    }
    for app_id in favorites {
        if !ordered.iter().any(|item| item == &app_id) {
            ordered.push(app_id);
        }
    }
    data.settings.favorite_order = ordered;
    save_data(&library_path, &data)?;
    Ok(data)
}

#[tauri::command]
fn clear_package_cache() -> Result<PackageCacheInfo, String> {
    let library_path = library_root()?;
    let cache_dir = package_cache_dir(&library_path);
    if cache_dir.exists() {
        fs::remove_dir_all(&cache_dir).map_err(error_message)?;
    }
    fs::create_dir_all(&cache_dir).map_err(error_message)?;
    package_cache_info(&library_path)
}

#[tauri::command]
fn get_transfer_progress(
    direction: String,
    app_id: String,
) -> Result<Option<TransferProgress>, String> {
    let key = transfer_key(&direction, &app_id);
    Ok(transfer_progress_store()
        .lock()
        .ok()
        .and_then(|store| store.get(&key).cloned()))
}

#[tauri::command]
fn debug_log(message: String) -> Result<(), String> {
    log_debug(&format!("frontend {}", message));
    Ok(())
}

#[tauri::command]
fn test_client_connection() -> Result<String, String> {
    let library_path = library_root()?;
    let data = load_or_create_data(&library_path)?;
    ensure_client_mode(&data.settings)?;
    let response = client_get(&data.settings.client, "/api/auth/test")?;

    if response.status == 200 {
        Ok("连接成功".to_string())
    } else if response.status == 401 {
        Err("认证失败，请检查用户名和密码".to_string())
    } else {
        Err(format!("连接失败，HTTP 状态：{}", response.status))
    }
}

#[tauri::command]
fn fetch_remote_apps() -> Result<Vec<ManagedApp>, String> {
    let library_path = library_root()?;
    let data = load_or_create_data(&library_path)?;
    ensure_client_mode(&data.settings)?;
    let response = client_get(&data.settings.client, "/api/apps")?;

    if response.status != 200 {
        return Err(format!(
            "获取服务端软件列表失败，HTTP 状态：{}",
            response.status
        ));
    }

    serde_json::from_slice(&response.body).map_err(error_message)
}

#[tauri::command]
fn list_review_apps() -> Result<Vec<ReviewItem>, String> {
    let library_path = library_root()?;
    list_review_items(&library_path)
}

#[tauri::command]
fn approve_review_app(review_id: String) -> Result<AppData, String> {
    let library_path = library_root()?;
    let mut data = load_or_create_data(&library_path)?;
    let item = list_review_items(&library_path)?
        .into_iter()
        .find(|item| item.id == review_id)
        .ok_or_else(|| "未审核软件不存在".to_string())?;

    let category = ensure_category_by_name(&library_path, &mut data, &item.category_name)?;
    let category_path = PathBuf::from(&category.path);
    fs::create_dir_all(&category_path).map_err(error_message)?;

    let source_path = PathBuf::from(normalize_incoming_path(&item.path));
    ensure_inside_review_dir(&library_path, &source_path)?;
    if !source_path.exists() {
        return Err("未审核软件文件不存在".to_string());
    }

    let target_path = category_path.join(sanitize_file_name(&item.file_name));
    if target_path.exists() {
        return Err("目标分类中已存在同名文件，请先处理后再审核".to_string());
    }
    let before_entries = snapshot_directory_entries(&category_path)?;
    fs::rename(&source_path, &target_path).map_err(error_message)?;
    let _ = fs::remove_file(review_meta_path(&source_path));

    let mut scan_paths = vec![target_path.clone()];
    if target_path
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case("zip"))
    {
        expand_zip_to_category(&target_path, &category_path)?;
        let _ = fs::remove_file(&target_path);
        scan_paths = new_directory_entries(&category_path, &before_entries)?;
    }

    cleanup_review_extracted_paths(&library_path, &item.extracted_paths)?;
    remove_review_apps_by_paths(&mut data, &item.extracted_paths);
    let result = scan_app_paths(&library_path, &mut data, &category, &scan_paths)?;
    save_data(&library_path, &result.data)?;
    Ok(result.data)
}

#[tauri::command]
fn reject_review_app(review_id: String) -> Result<Vec<ReviewItem>, String> {
    let library_path = library_root()?;
    let item = list_review_items(&library_path)?
        .into_iter()
        .find(|item| item.id == review_id)
        .ok_or_else(|| "未审核软件不存在".to_string())?;
    let source_path = PathBuf::from(normalize_incoming_path(&item.path));
    ensure_inside_review_dir(&library_path, &source_path)?;
    if source_path.exists() {
        fs::remove_file(&source_path).map_err(error_message)?;
    }
    let _ = fs::remove_file(review_meta_path(&source_path));
    cleanup_review_extracted_paths(&library_path, &item.extracted_paths)?;
    let mut data = load_or_create_data(&library_path)?;
    remove_review_apps_by_paths(&mut data, &item.extracted_paths);
    save_data(&library_path, &data)?;
    list_review_items(&library_path)
}

#[tauri::command]
async fn download_remote_app(
    app_handle: tauri::AppHandle,
    app_id: String,
    app_name: Option<String>,
) -> Result<AppData, String> {
    let display_name = app_name.as_deref().unwrap_or("远程软件").to_string();
    tauri::async_runtime::spawn_blocking(move || {
        let result = download_remote_app_inner(&app_handle, &app_id, &display_name);
        if result.is_err() {
            emit_transfer_progress(
                Some(&app_handle),
                "download",
                &app_id,
                &display_name,
                0,
                0,
                Instant::now(),
                "error",
            );
        }
        result
    })
    .await
    .map_err(|error| format!("下载任务异常结束：{error}"))?
}

fn download_remote_app_inner(
    app_handle: &tauri::AppHandle,
    app_id: &str,
    display_name: &str,
) -> Result<AppData, String> {
    let library_path = library_root()?;
    let mut data = load_or_create_data(&library_path)?;
    ensure_client_mode(&data.settings)?;
    let path = format!("/api/apps/{app_id}/download");
    let temp_download_path = std::env::temp_dir().join(format!(
        "appmanager-download-{app_id}-{}.part",
        next_temp_file_sequence()
    ));
    log_debug(&format!(
        "client download command start app_id={} name={} target_temp={}",
        app_id,
        display_name,
        native_path_to_string(&temp_download_path)
    ));
    let response = client_get_to_file(
        &data.settings.client,
        &path,
        "download",
        app_id,
        display_name,
        Some(app_handle),
        &temp_download_path,
    )?;
    log_debug(&format!(
        "client download response app_id={} status={} transferred={} total={} body_path={}",
        app_id,
        response.status,
        response.transferred,
        response.total,
        native_path_to_string(&response.body_path)
    ));

    if response.status != 200 {
        let error_message = response_error_message_from_file(&response.body_path);
        let _ = fs::remove_file(&response.body_path);
        return Err(format!(
            "下载失败，HTTP 状态：{}{}",
            response.status, error_message
        ));
    }

    let file_name = header_value(&response.headers, "x-appmanager-filename")
        .unwrap_or_else(|| format!("{app_id}.bin"));
    let category_name = header_value(&response.headers, "x-appmanager-category")
        .unwrap_or_else(|| "来自服务端".to_string());
    let category = ensure_category_by_name(&library_path, &mut data, &category_name)?;
    let category_path = PathBuf::from(&category.path);
    fs::create_dir_all(&category_path).map_err(error_message)?;

    let before_entries = snapshot_directory_entries(&category_path)?;
    let target_path = category_path.join(sanitize_file_name(&file_name));
    if target_path.exists() {
        if target_path.is_dir() {
            fs::remove_dir_all(&target_path).map_err(error_message)?;
        } else {
            fs::remove_file(&target_path).map_err(error_message)?;
        }
    }
    fs::rename(&response.body_path, &target_path)
        .or_else(|_| {
            fs::copy(&response.body_path, &target_path)?;
            fs::remove_file(&response.body_path)
        })
        .map_err(error_message)?;
    log_debug(&format!(
        "client download saved app_id={} target={}",
        app_id,
        native_path_to_string(&target_path)
    ));

    let mut scan_paths = vec![target_path.clone()];
    if target_path
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case("zip"))
    {
        emit_transfer_progress(
            Some(app_handle),
            "download",
            app_id,
            &file_name,
            response.transferred,
            response.total,
            Instant::now(),
            "extracting",
        );
        expand_zip_to_category(&target_path, &category_path)?;
        let _ = fs::remove_file(&target_path);
        scan_paths = new_directory_entries(&category_path, &before_entries)?;
    }

    emit_transfer_progress(
        Some(app_handle),
        "download",
        app_id,
        &file_name,
        response.transferred,
        response.total,
        Instant::now(),
        "installing",
    );
    let result = scan_app_paths(&library_path, &mut data, &category, &scan_paths)?;
    save_data(&library_path, &result.data)?;
    emit_transfer_progress(
        Some(app_handle),
        "download",
        app_id,
        &file_name,
        response.transferred,
        response.total,
        Instant::now(),
        "done",
    );
    Ok(result.data)
}

#[tauri::command]
async fn upload_app_to_server(
    app_handle: tauri::AppHandle,
    app_id: String,
    app_name: Option<String>,
) -> Result<String, String> {
    let display_name = app_name.as_deref().unwrap_or("本地软件").to_string();
    tauri::async_runtime::spawn_blocking(move || {
        let result = upload_app_to_server_inner(&app_handle, &app_id);
        if result.is_err() {
            emit_transfer_progress(
                Some(&app_handle),
                "upload",
                &app_id,
                &display_name,
                0,
                0,
                Instant::now(),
                "error",
            );
        }
        result
    })
    .await
    .map_err(|error| format!("上传任务异常结束：{error}"))?
}

fn upload_app_to_server_inner(
    app_handle: &tauri::AppHandle,
    app_id: &str,
) -> Result<String, String> {
    let library_path = library_root()?;
    let data = load_or_create_data(&library_path)?;
    ensure_client_mode(&data.settings)?;
    let app = data
        .apps
        .iter()
        .find(|item| item.id == app_id)
        .cloned()
        .ok_or_else(|| "软件不存在".to_string())?;
    emit_transfer_progress(
        Some(app_handle),
        "upload",
        &app.id,
        &app.name,
        0,
        0,
        Instant::now(),
        "packing",
    );
    let (upload_path, file_name) = prepare_download_file(&app)?;
    let result = client_upload(
        &data.settings.client,
        &app,
        &upload_path,
        &file_name,
        app_handle,
    );
    if upload_path.starts_with(std::env::temp_dir()) {
        let _ = fs::remove_file(upload_path);
    }
    result
}

#[tauri::command]
fn launch_app(app_id: String) -> Result<LaunchResult, String> {
    launch_app_inner(app_id, false)
}

#[tauri::command]
fn launch_app_as_admin(app_id: String) -> Result<LaunchResult, String> {
    launch_app_inner(app_id, true)
}

fn launch_app_inner(app_id: String, as_admin: bool) -> Result<LaunchResult, String> {
    let library_path = library_root()?;
    let mut data = load_or_create_data(&library_path)?;
    let app = data
        .apps
        .iter_mut()
        .find(|item| item.id == app_id)
        .ok_or_else(|| "软件不存在".to_string())?;

    let executable_path = app
        .executable_path
        .clone()
        .ok_or_else(|| "该软件还没有设置启动程序".to_string())?;

    let executable = PathBuf::from(&executable_path);
    if !executable.exists() {
        return Err("启动程序不存在，请重新扫描或编辑软件信息".to_string());
    }

    if as_admin {
        launch_process_as_admin(&executable)?;
    } else {
        hidden_command(&executable)
            .current_dir(executable.parent().unwrap_or_else(|| Path::new(".")))
            .spawn()
            .map_err(error_message)?;
    }

    app.launch_count += 1;
    app.last_launched_at = Some(now());
    let result = LaunchResult {
        app_id: app.id.clone(),
        launch_count: app.launch_count,
        last_launched_at: app.last_launched_at.unwrap_or_default(),
    };

    save_data(&library_path, &data)?;
    Ok(result)
}

#[cfg(windows)]
fn launch_process_as_admin(executable: &Path) -> Result<(), String> {
    let parent = executable.parent().unwrap_or_else(|| Path::new("."));
    let verb = wide_null("runas");
    let file = wide_null(executable.as_os_str());
    let directory = wide_null(parent.as_os_str());
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            file.as_ptr(),
            std::ptr::null(),
            directory.as_ptr(),
            1,
        )
    };
    let code = result as isize;
    if code <= 32 {
        return Err(format!("管理员权限启动失败，系统错误码：{code}"));
    }
    Ok(())
}

#[cfg(windows)]
fn wide_null(value: impl AsRef<OsStr>) -> Vec<u16> {
    value.as_ref().encode_wide().chain(Some(0)).collect()
}

#[cfg(windows)]
#[link(name = "shell32")]
extern "system" {
    fn ShellExecuteW(
        hwnd: *mut std::ffi::c_void,
        lpoperation: *const u16,
        lpfile: *const u16,
        lpparameters: *const u16,
        lpdirectory: *const u16,
        nshowcmd: i32,
    ) -> *mut std::ffi::c_void;
}

#[cfg(not(windows))]
fn launch_process_as_admin(_executable: &Path) -> Result<(), String> {
    Err("管理员权限启动仅支持 Windows".to_string())
}

#[tauri::command]
fn reveal_path(path: String) -> Result<(), String> {
    let target = PathBuf::from(normalize_incoming_path(&path));
    if !target.exists() {
        return Err("路径不存在".to_string());
    }

    let target = target.canonicalize().map_err(error_message)?;
    hidden_command("explorer.exe")
        .arg(native_path_to_string(&target))
        .spawn()
        .map_err(error_message)?;

    Ok(())
}

fn delete_app_files(library_path: &Path, app: &ManagedApp) -> Result<(), String> {
    let folder_path = PathBuf::from(normalize_incoming_path(&app.folder_path));
    ensure_inside_apps_dir(library_path, &folder_path)?;

    let executable_path = app
        .executable_path
        .as_ref()
        .map(|path| PathBuf::from(normalize_incoming_path(path)));

    if let Some(executable_path) = executable_path {
        ensure_inside_apps_dir(library_path, &executable_path)?;
        if folder_path == executable_path.parent().unwrap_or_else(|| Path::new("")) {
            if executable_path.exists() {
                fs::remove_file(&executable_path).map_err(error_message)?;
            }
            return Ok(());
        }
    }

    if folder_path.exists() {
        fs::remove_dir_all(&folder_path).map_err(error_message)?;
    }

    Ok(())
}

fn move_app_files(
    old_folder_path: &Path,
    old_executable_path: Option<&Path>,
    target_category_path: &Path,
) -> Result<(PathBuf, Option<PathBuf>), String> {
    if let Some(old_executable_path) = old_executable_path {
        if old_executable_path.exists()
            && old_folder_path
                == old_executable_path
                    .parent()
                    .unwrap_or_else(|| Path::new(""))
        {
            let file_name = old_executable_path
                .file_name()
                .ok_or_else(|| "无法读取软件文件名".to_string())?;
            let new_executable_path = target_category_path.join(file_name);
            if new_executable_path.exists() {
                return Err("目标分类中已存在同名软件文件".to_string());
            }
            fs::rename(old_executable_path, &new_executable_path).map_err(error_message)?;
            return Ok((
                target_category_path.to_path_buf(),
                Some(new_executable_path),
            ));
        }
    }

    let folder_name = old_folder_path
        .file_name()
        .ok_or_else(|| "无法读取软件文件夹名".to_string())?;
    let new_folder_path = target_category_path.join(folder_name);
    if new_folder_path.exists() {
        return Err("目标分类中已存在同名软件文件夹".to_string());
    }

    fs::rename(old_folder_path, &new_folder_path).map_err(error_message)?;

    let new_executable_path = old_executable_path.and_then(|path| {
        path.strip_prefix(old_folder_path)
            .ok()
            .map(|relative| new_folder_path.join(relative))
    });

    Ok((new_folder_path, new_executable_path))
}

fn read_image_as_data_url(path: &str) -> Result<String, String> {
    let image_path = PathBuf::from(normalize_incoming_path(path));
    if !image_path.exists() {
        return Err("图标文件不存在".to_string());
    }

    let mime = match image_path
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| value.to_ascii_lowercase())
        .as_deref()
    {
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("ico") => "image/x-icon",
        Some("bmp") => "image/bmp",
        _ => return Err("图标仅支持 png、jpg、jpeg、gif、webp、ico、bmp".to_string()),
    };

    let bytes = fs::read(image_path).map_err(error_message)?;
    Ok(format!("data:{mime};base64,{}", base64_encode(&bytes)))
}

fn validate_image_data_url(value: &str) -> Result<String, String> {
    let clean_value = value.trim();
    if clean_value.len() > 4 * 1024 * 1024 {
        return Err("图标图片不能超过 4 MB".to_string());
    }

    decode_image_data_url(clean_value)?;

    Ok(clean_value.to_string())
}

fn decode_image_data_url(value: &str) -> Result<(&str, Vec<u8>), String> {
    let Some((mime, payload)) = value.trim().split_once(";base64,") else {
        return Err("图标图片格式无效".to_string());
    };
    icon_extension_from_mime(mime)?;
    let bytes = base64_decode(payload)?;
    if bytes.is_empty() {
        return Err("图标图片内容无效".to_string());
    }
    Ok((mime, bytes))
}

fn icon_extension_from_mime(mime: &str) -> Result<&'static str, String> {
    match mime {
        "data:image/png" => Ok("png"),
        "data:image/jpeg" => Ok("jpg"),
        "data:image/gif" => Ok("gif"),
        "data:image/webp" => Ok("webp"),
        "data:image/x-icon" | "data:image/vnd.microsoft.icon" => Ok("ico"),
        "data:image/bmp" => Ok("bmp"),
        _ => Err("图标仅支持 png、jpg、jpeg、gif、webp、ico、bmp".to_string()),
    }
}

fn set_windows_autostart(enabled: bool) -> Result<(), String> {
    let app_path = std::env::current_exe().map_err(error_message)?;
    let app_path = native_path_to_string(&app_path);
    let key_path = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";

    let status = if enabled {
        hidden_command("reg.exe")
            .args(["add", key_path, "/v", "AppManager", "/t", "REG_SZ", "/d"])
            .arg(app_path)
            .args(["/f"])
            .status()
            .map_err(error_message)?
    } else {
        hidden_command("reg.exe")
            .args(["delete", key_path, "/v", "AppManager", "/f"])
            .status()
            .map_err(error_message)?
    };

    if enabled && !status.success() {
        return Err("写入开机自启失败".to_string());
    }

    Ok(())
}

fn sync_server(settings: &Settings, app_handle: &tauri::AppHandle) -> Result<(), String> {
    if settings.run_mode == "server" {
        if settings.server.username.trim().is_empty()
            || (settings.server.password_hash.is_empty() && settings.server.password.is_empty())
        {
            stop_server();
            return Err("请先设置服务端用户名和密码".to_string());
        }

        start_server(settings.server.clone(), app_handle.clone())
    } else {
        stop_server();
        Ok(())
    }
}

fn server_runtime() -> &'static Mutex<Option<ServerRuntime>> {
    SERVER_RUNTIME.get_or_init(|| Mutex::new(None))
}

fn server_clients() -> &'static Mutex<HashMap<String, ServerClientInfo>> {
    SERVER_CLIENTS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn transfer_progress_store() -> &'static Mutex<HashMap<String, TransferProgress>> {
    TRANSFER_PROGRESS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn transfer_speed_samples() -> &'static Mutex<HashMap<String, TransferSpeedSample>> {
    TRANSFER_SPEED_SAMPLES.get_or_init(|| Mutex::new(HashMap::new()))
}

fn transfer_key(direction: &str, app_id: &str) -> String {
    format!("{direction}-{app_id}")
}

fn current_server_status() -> ServerStatus {
    let runtime = server_runtime().lock().ok();
    let clients = recent_server_clients();
    if let Some(Some(runtime)) = runtime.as_deref() {
        ServerStatus {
            running: !runtime.stop.load(Ordering::SeqCst),
            host: runtime.host.clone(),
            port: runtime.port,
            clients,
        }
    } else {
        ServerStatus {
            running: false,
            host: String::new(),
            port: 0,
            clients,
        }
    }
}

fn recent_server_clients() -> Vec<ServerClientInfo> {
    let current = now();
    let keep_cutoff = current.saturating_sub(SERVER_CLIENT_KEEP_SECONDS);
    let online_cutoff = current.saturating_sub(SERVER_CLIENT_ONLINE_SECONDS);
    let Ok(mut clients) = server_clients().lock() else {
        return Vec::new();
    };
    clients.retain(|_, client| client.last_seen_at >= keep_cutoff);
    let mut values = clients.values().cloned().collect::<Vec<_>>();
    for client in &mut values {
        client.online = client.last_seen_at >= online_cutoff;
    }
    values.sort_by_key(|client| Reverse(client.last_seen_at));
    values.truncate(12);
    values
}

fn start_server(config: ServerConfig, app_handle: tauri::AppHandle) -> Result<(), String> {
    stop_server();

    let listener = TcpListener::bind(format!("{}:{}", config.host, config.port))
        .map_err(|error| format!("鏈嶅姟绔洃鍚け璐ワ細{error}"))?;
    listener.set_nonblocking(true).map_err(error_message)?;

    let stop = Arc::new(AtomicBool::new(false));
    let stop_for_thread = Arc::clone(&stop);
    let host = config.host.clone();
    let port = config.port;

    thread::spawn(move || {
        while !stop_for_thread.load(Ordering::SeqCst) {
            match listener.accept() {
                Ok((stream, _)) => {
                    let config = config.clone();
                    let app_handle = app_handle.clone();
                    thread::spawn(move || {
                        handle_http_stream(stream, config, app_handle);
                    });
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(std::time::Duration::from_millis(80));
                }
                Err(_) => break,
            }
        }
    });

    if let Ok(mut runtime) = server_runtime().lock() {
        *runtime = Some(ServerRuntime { stop, host, port });
    }

    Ok(())
}

fn stop_server() {
    if let Ok(mut runtime) = server_runtime().lock() {
        if let Some(runtime) = runtime.take() {
            runtime.stop.store(true, Ordering::SeqCst);
        }
    }
}

fn handle_http_stream(mut stream: TcpStream, config: ServerConfig, app_handle: tauri::AppHandle) {
    if let Err(error) = stream.set_nonblocking(false) {
        log_debug(&format!(
            "server stream set blocking failed error={}",
            error
        ));
    }

    let mut buffer = [0; 16 * 1024];
    let read_count = match stream.read(&mut buffer) {
        Ok(count) => count,
        Err(_) => return,
    };

    let request = String::from_utf8_lossy(&buffer[..read_count]);
    let mut lines = request.lines();
    let request_line = lines.next().unwrap_or_default();
    let parts = request_line.split_whitespace().collect::<Vec<_>>();
    if parts.len() < 2 {
        write_json_response(&mut stream, 400, r#"{"error":"bad request"}"#);
        return;
    }

    let method = parts[0];
    let path = parts[1].split('?').next().unwrap_or(parts[1]);
    let headers = parse_headers(&request);
    let authorized = path == "/api/server-info" || is_authorized(&headers, &config);
    if authorized && !path.ends_with("/progress") {
        remember_server_client(&stream, &headers, path);
    }

    if method == "POST" && path == "/api/apps/upload" {
        if !authorized {
            write_json_response(&mut stream, 401, r#"{"error":"unauthorized"}"#);
            return;
        }
        handle_upload_request(
            &mut stream,
            &headers,
            &buffer[..read_count],
            config,
            Some(&app_handle),
        );
        return;
    }

    if method != "GET" {
        write_json_response(&mut stream, 405, r#"{"error":"method not allowed"}"#);
        return;
    }

    if !authorized {
        write_json_response(&mut stream, 401, r#"{"error":"unauthorized"}"#);
        return;
    }

    if path.starts_with("/api/apps/") && path.ends_with("/download") {
        handle_download_request(&mut stream, path, config, Some(&app_handle));
        return;
    }

    if path.starts_with("/api/apps/") && path.ends_with("/progress") {
        handle_transfer_progress_request(&mut stream, path);
        return;
    }

    let body = match path {
        "/api/server-info" => serde_json::json!({
            "name": "AppManager",
            "mode": "server",
            "authRequired": true,
            "allowDownloads": config.allow_downloads
        })
        .to_string(),
        "/api/auth/test" => serde_json::json!({ "ok": true }).to_string(),
        "/api/categories" => match load_server_data() {
            Ok(data) => {
                serde_json::to_string(&data.categories).unwrap_or_else(|_| "[]".to_string())
            }
            Err(error) => {
                write_json_response(
                    &mut stream,
                    500,
                    &serde_json::json!({ "error": error }).to_string(),
                );
                return;
            }
        },
        "/api/apps" => match load_server_data() {
            Ok(data) => serde_json::to_string(&data.apps).unwrap_or_else(|_| "[]".to_string()),
            Err(error) => {
                write_json_response(
                    &mut stream,
                    500,
                    &serde_json::json!({ "error": error }).to_string(),
                );
                return;
            }
        },
        _ => {
            write_json_response(&mut stream, 404, r#"{"error":"not found"}"#);
            return;
        }
    };

    write_json_response(&mut stream, 200, &body);
}

fn remember_server_client(stream: &TcpStream, headers: &[(String, String)], path: &str) {
    let address = stream
        .peer_addr()
        .map(|addr| addr.ip().to_string())
        .unwrap_or_else(|_| "unknown".to_string());
    let username = headers
        .iter()
        .find(|(name, _)| name == "x-appmanager-username")
        .map(|(_, value)| value.clone())
        .unwrap_or_else(|| "anonymous".to_string());
    let client = ServerClientInfo {
        address: address.clone(),
        username,
        last_path: path.to_string(),
        last_seen_at: now(),
        online: true,
    };

    if let Ok(mut clients) = server_clients().lock() {
        clients.insert(address, client);
    }
}

fn handle_download_request(
    stream: &mut TcpStream,
    path: &str,
    config: ServerConfig,
    app_handle: Option<&tauri::AppHandle>,
) {
    if !config.allow_downloads {
        write_json_response(stream, 403, r#"{"error":"downloads disabled"}"#);
        return;
    }

    let app_id = path
        .trim_start_matches("/api/apps/")
        .trim_end_matches("/download")
        .trim_matches('/');
    log_debug(&format!("server download request start app_id={app_id}"));

    let data = match load_server_data() {
        Ok(data) => data,
        Err(error) => {
            write_json_response(
                stream,
                500,
                &serde_json::json!({ "error": error }).to_string(),
            );
            return;
        }
    };

    let app = match data.apps.iter().find(|item| item.id == app_id) {
        Some(app) => app,
        None => {
            write_json_response(stream, 404, r#"{"error":"app not found"}"#);
            return;
        }
    };

    emit_transfer_progress(
        app_handle,
        "download",
        &app.id,
        &app.name,
        0,
        0,
        Instant::now(),
        "packing",
    );
    log_debug(&format!(
        "server download packing progress init app_id={} name={}",
        app.id, app.name
    ));
    let (download_path, file_name) = match prepare_download_file_with_progress(app, app_handle) {
        Ok(value) => value,
        Err(error) => {
            log_debug(&format!(
                "server prepare download failed app_id={app_id} error={error}"
            ));
            write_json_response(
                stream,
                500,
                &serde_json::json!({ "error": error }).to_string(),
            );
            return;
        }
    };
    let download_size = fs::metadata(&download_path)
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    log_debug(&format!(
        "server download package ready app_id={} file={} path={} size={}",
        app_id,
        file_name,
        native_path_to_string(&download_path),
        download_size
    ));

    let result = write_file_response(
        stream,
        200,
        &download_path,
        &[
            ("Content-Type", "application/octet-stream"),
            ("X-AppManager-Filename", &file_name),
            ("X-AppManager-Category", &app.category_name),
        ],
    );

    if let Err(error) = result {
        log_debug(&format!(
            "server download send failed app_id={app_id} file={file_name} error={error}"
        ));
    } else {
        log_debug(&format!(
            "server download send done app_id={app_id} file={file_name} size={download_size}"
        ));
    }

    if download_path.starts_with(std::env::temp_dir()) {
        let _ = fs::remove_file(download_path);
    }
}

fn handle_transfer_progress_request(stream: &mut TcpStream, path: &str) {
    let app_id = path
        .trim_start_matches("/api/apps/")
        .trim_end_matches("/progress")
        .trim_matches('/');
    let key = transfer_key("download", app_id);
    let progress = transfer_progress_store()
        .lock()
        .ok()
        .and_then(|store| store.get(&key).cloned());
    if progress.is_none() {
        log_debug(&format!(
            "server progress request app_id={} missing",
            app_id
        ));
    }
    let body = progress
        .and_then(|progress| serde_json::to_string(&progress).ok())
        .unwrap_or_else(|| "null".to_string());
    write_json_response(stream, 200, &body);
}

fn write_file_response(
    stream: &mut TcpStream,
    status: u16,
    path: &Path,
    headers: &[(&str, &str)],
) -> Result<(), String> {
    let mut file = fs::File::open(path).map_err(error_message)?;
    let file_size = file.metadata().map_err(error_message)?.len();
    log_debug(&format!(
        "server file response start status={} path={} size={}",
        status,
        native_path_to_string(path),
        file_size
    ));
    let status_text = match status {
        200 => "OK",
        403 => "Forbidden",
        404 => "Not Found",
        500 => "Internal Server Error",
        _ => "OK",
    };

    let mut response = format!(
        "HTTP/1.1 {status} {status_text}\r\nContent-Length: {file_size}\r\nConnection: close\r\n"
    );
    for (name, value) in headers {
        response.push_str(name);
        response.push_str(": ");
        response.push_str(value);
        response.push_str("\r\n");
    }
    response.push_str("\r\n");
    stream
        .write_all(response.as_bytes())
        .map_err(error_message)?;

    let mut buffer = [0u8; 64 * 1024];
    let mut sent = 0u64;
    let mut chunk_index = 0u64;
    let mut next_progress_log = 10 * 1024 * 1024u64;
    loop {
        let count = file.read(&mut buffer).map_err(error_message)?;
        if count == 0 {
            log_debug(&format!(
                "server file response read eof path={} sent={} expected={}",
                native_path_to_string(path),
                sent,
                file_size
            ));
            break;
        }
        if let Err(error) = stream.write_all(&buffer[..count]) {
            log_debug(&format!(
                "server file response body write failed path={} sent={} next_chunk={} error={}",
                native_path_to_string(path),
                sent,
                count,
                error
            ));
            return Err(error_message(error));
        }
        sent += count as u64;
        chunk_index += 1;
        if chunk_index <= 3 || sent >= next_progress_log || sent == file_size {
            log_debug(&format!(
                "server file response progress path={} chunk={} sent={} expected={}",
                native_path_to_string(path),
                chunk_index,
                sent,
                file_size
            ));
            while sent >= next_progress_log {
                next_progress_log += 10 * 1024 * 1024;
            }
        }
    }
    stream.flush().map_err(error_message)?;
    let _ = stream.shutdown(Shutdown::Write);

    if sent != file_size {
        log_debug(&format!(
            "server file response size mismatch path={} sent={} expected={}",
            native_path_to_string(path),
            sent,
            file_size
        ));
        return Err(format!(
            "鏈嶅姟绔彂閫佷笉瀹屾暣锛氬凡鍙戯拷?{sent} / {file_size} 瀛楄妭"
        ));
    }

    log_debug(&format!(
        "server file response done path={} sent={}",
        native_path_to_string(path),
        sent
    ));
    Ok(())
}

fn handle_upload_request(
    stream: &mut TcpStream,
    headers: &[(String, String)],
    initial_buffer: &[u8],
    config: ServerConfig,
    app_handle: Option<&tauri::AppHandle>,
) {
    if !config.allow_downloads {
        write_json_response(stream, 403, r#"{"error":"uploads disabled"}"#);
        return;
    }

    let content_length = match header_value(headers, "content-length")
        .and_then(|value| value.parse::<usize>().ok())
    {
        Some(value) if value > 0 => value,
        _ => {
            write_json_response(stream, 400, r#"{"error":"missing content length"}"#);
            return;
        }
    };
    let file_name = header_value(headers, "x-appmanager-filename")
        .unwrap_or_else(|| "uploaded.zip".to_string());
    let category_name =
        header_value(headers, "x-appmanager-category").unwrap_or_else(|| REVIEW_DIR.to_string());
    let app_id = format!("server-upload-{}", make_id(&file_name));
    let app_name = Path::new(&file_name)
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or(&file_name)
        .to_string();

    let split_at = match initial_buffer
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
    {
        Some(value) => value + 4,
        None => {
            write_json_response(stream, 400, r#"{"error":"bad request"}"#);
            return;
        }
    };

    let library_path = match library_root() {
        Ok(value) => value,
        Err(error) => {
            write_json_response(
                stream,
                500,
                &serde_json::json!({ "error": error }).to_string(),
            );
            return;
        }
    };
    let review_path = review_dir_path(&library_path);
    if let Err(error) = fs::create_dir_all(&review_path).map_err(error_message) {
        write_json_response(
            stream,
            500,
            &serde_json::json!({ "error": error }).to_string(),
        );
        return;
    }

    let clean_file_name = unique_file_name(&review_path, &sanitize_file_name(&file_name));
    let target_path = review_path.join(clean_file_name);
    let meta_path = review_meta_path(&target_path);
    emit_transfer_progress(
        app_handle,
        "upload",
        &app_id,
        &app_name,
        0,
        content_length as u64,
        Instant::now(),
        "running",
    );

    let mut file = match fs::File::create(&target_path) {
        Ok(value) => value,
        Err(error) => {
            write_json_response(
                stream,
                500,
                &serde_json::json!({ "error": error.to_string() }).to_string(),
            );
            return;
        }
    };

    let started_at = Instant::now();
    let mut last_emit = Instant::now();
    let mut written = 0usize;
    let initial_body = &initial_buffer[split_at..];
    if !initial_body.is_empty() {
        let take = initial_body.len().min(content_length);
        if let Err(error) = file.write_all(&initial_body[..take]) {
            write_json_response(
                stream,
                500,
                &serde_json::json!({ "error": error.to_string() }).to_string(),
            );
            return;
        }
        written += take;
        emit_transfer_progress(
            app_handle,
            "upload",
            &app_id,
            &app_name,
            written as u64,
            content_length as u64,
            started_at,
            "running",
        );
    }

    let mut chunk = [0u8; 64 * 1024];
    while written < content_length {
        let expected = (content_length - written).min(chunk.len());
        let count = match stream.read(&mut chunk[..expected]) {
            Ok(0) => break,
            Ok(count) => count,
            Err(error) => {
                write_json_response(
                    stream,
                    500,
                    &serde_json::json!({ "error": error.to_string() }).to_string(),
                );
                return;
            }
        };
        if let Err(error) = file.write_all(&chunk[..count]) {
            write_json_response(
                stream,
                500,
                &serde_json::json!({ "error": error.to_string() }).to_string(),
            );
            return;
        }
        written += count;
        if last_emit.elapsed().as_millis() > 350 || written == content_length {
            emit_transfer_progress(
                app_handle,
                "upload",
                &app_id,
                &app_name,
                written as u64,
                content_length as u64,
                started_at,
                "running",
            );
            last_emit = Instant::now();
        }
    }

    if written != content_length {
        let _ = fs::remove_file(&target_path);
        emit_transfer_progress(
            app_handle,
            "upload",
            &app_id,
            &app_name,
            written as u64,
            content_length as u64,
            started_at,
            "error",
        );
        write_json_response(stream, 400, r#"{"error":"incomplete upload"}"#);
        return;
    }

    log_debug(&format!(
        "server upload saved file={} bytes={}",
        native_path_to_string(&target_path),
        written
    ));

    let (scan_added, scan_updated, extracted_paths) =
        match register_uploaded_review_app(&library_path, &review_path, &target_path) {
            Ok(value) => value,
            Err(error) => {
                log_debug(&format!(
                    "server upload register failed file={} error={}",
                    native_path_to_string(&target_path),
                    error
                ));
                emit_transfer_progress(
                    app_handle,
                    "upload",
                    &app_id,
                    &app_name,
                    written as u64,
                    content_length as u64,
                    started_at,
                    "error",
                );
                write_json_response(
                    stream,
                    500,
                    &serde_json::json!({ "error": error }).to_string(),
                );
                return;
            }
        };
    emit_transfer_progress(
        app_handle,
        "upload",
        &app_id,
        &app_name,
        written as u64,
        content_length as u64,
        started_at,
        "installing",
    );

    let metadata = serde_json::json!({
        "categoryName": category_name,
        "uploadedAt": now(),
        "registeredInReviewCategory": true,
        "extractedPaths": extracted_paths
    });
    let _ = fs::write(
        &meta_path,
        serde_json::to_string_pretty(&metadata).unwrap_or_default(),
    );
    cleanup_uploaded_review_package(&target_path, &meta_path);
    log_debug(&format!(
        "server upload done file={} registered=true added={} updated={}",
        native_path_to_string(&target_path),
        scan_added,
        scan_updated
    ));
    emit_transfer_progress(
        app_handle,
        "upload",
        &app_id,
        &app_name,
        written as u64,
        content_length as u64,
        started_at,
        "done",
    );
    write_json_response(stream, 200, r#"{"ok":true}"#);
}

fn cleanup_uploaded_review_package(package_path: &Path, meta_path: &Path) {
    if package_path.exists() {
        match fs::remove_file(package_path) {
            Ok(_) => log_debug(&format!(
                "server upload cleanup package={}",
                native_path_to_string(package_path)
            )),
            Err(error) => log_debug(&format!(
                "server upload cleanup package failed path={} error={}",
                native_path_to_string(package_path),
                error
            )),
        }
    }
    if meta_path.exists() {
        match fs::remove_file(meta_path) {
            Ok(_) => log_debug(&format!(
                "server upload cleanup meta={}",
                native_path_to_string(meta_path)
            )),
            Err(error) => log_debug(&format!(
                "server upload cleanup meta failed path={} error={}",
                native_path_to_string(meta_path),
                error
            )),
        }
    }
}

fn register_uploaded_review_app(
    library_path: &Path,
    review_path: &Path,
    target_path: &Path,
) -> Result<(usize, usize, Vec<String>), String> {
    let mut data = load_or_create_data(library_path)?;
    let category = ensure_category_by_name(library_path, &mut data, REVIEW_DIR)?;
    fs::create_dir_all(review_path).map_err(error_message)?;

    let before_entries = snapshot_directory_entries(review_path)?;
    let mut scan_paths = vec![target_path.to_path_buf()];
    if target_path
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case("zip"))
    {
        expand_zip_to_category(target_path, review_path)?;
        scan_paths = new_directory_entries(review_path, &before_entries)?;
    }

    let extracted_paths = scan_paths
        .iter()
        .filter(|path| path.is_dir())
        .map(|path| path_to_string(path))
        .collect::<Vec<_>>();
    if extracted_paths.is_empty() {
        return Err("上传包没有解压出可扫描的软件文件夹".to_string());
    }
    let result = scan_app_paths(library_path, &mut data, &category, &scan_paths)?;
    if result.added == 0 && result.updated == 0 {
        return Err("上传包已解压，但没有扫描到可加入未审核软件的软件目录".to_string());
    }
    save_data(library_path, &result.data)?;
    Ok((result.added, result.updated, extracted_paths))
}

fn prepare_download_file(app: &ManagedApp) -> Result<(PathBuf, String), String> {
    prepare_download_file_internal(app, None)
}

fn prepare_download_file_with_progress(
    app: &ManagedApp,
    app_handle: Option<&tauri::AppHandle>,
) -> Result<(PathBuf, String), String> {
    prepare_download_file_internal(app, app_handle)
}

fn prepare_download_file_internal(
    app: &ManagedApp,
    app_handle: Option<&tauri::AppHandle>,
) -> Result<(PathBuf, String), String> {
    let library_path = library_root()?;
    let folder_path = PathBuf::from(normalize_incoming_path(&app.folder_path));

    let source_path = if folder_path.is_dir() {
        folder_path
    } else if let Some(executable_path) = app
        .executable_path
        .as_ref()
        .map(|path| PathBuf::from(normalize_incoming_path(path)))
    {
        executable_path
            .parent()
            .map(|path| path.to_path_buf())
            .ok_or_else(|| "无法读取软件文件夹名".to_string())?
    } else {
        return Err("软件文件不存在".to_string());
    };

    if !source_path.exists() {
        return Err("软件文件不存在".to_string());
    }

    let package_name = source_path
        .file_stem()
        .or_else(|| source_path.file_name())
        .and_then(|value| value.to_str())
        .unwrap_or(&app.name);
    let file_name = format!("{}.zip", sanitize_file_name(package_name));
    let source_signature = package_source_signature(&source_path)?;
    let cache_path = package_cache_path(&library_path, &app.id, &source_signature, &file_name)?;
    if cache_path.exists() {
        log_debug(&format!(
            "download package cache hit app_id={} source={} cache={}",
            app.id,
            native_path_to_string(&source_path),
            native_path_to_string(&cache_path)
        ));
        emit_transfer_progress(
            app_handle,
            "download",
            &app.id,
            &app.name,
            source_signature.total_bytes,
            source_signature.total_bytes,
            Instant::now(),
            "packing",
        );
        return Ok((cache_path, file_name));
    }

    cleanup_package_cache_for_app(&library_path, &app.id);
    let zip_path = std::env::temp_dir().join(format!(
        "appmanager-{}-{}-{file_name}",
        app.id,
        next_temp_file_sequence()
    ));
    if zip_path.exists() {
        let _ = fs::remove_file(&zip_path);
    }

    let progress = ZipProgress {
        app_handle,
        direction: "download",
        app_id: &app.id,
        app_name: &app.name,
        status: "packing",
    };
    if let Err(error) = create_stored_zip(
        &source_path,
        &zip_path,
        Some(progress),
        Some(&source_signature),
    ) {
        let _ = fs::remove_file(&zip_path);
        return Err(format!(
            "打包软件失败：{}：{}",
            native_path_to_string(&source_path),
            error
        ));
    }

    if let Some(parent) = cache_path.parent() {
        fs::create_dir_all(parent).map_err(error_message)?;
    }
    fs::rename(&zip_path, &cache_path)
        .or_else(|_| {
            fs::copy(&zip_path, &cache_path)?;
            fs::remove_file(&zip_path)
        })
        .map_err(error_message)?;
    log_debug(&format!(
        "download package cache saved app_id={} cache={}",
        app.id,
        native_path_to_string(&cache_path)
    ));

    Ok((cache_path, file_name))
}

fn load_server_data() -> Result<AppData, String> {
    let library_path = library_root()?;
    load_or_create_data(&library_path)
}

fn parse_headers(request: &str) -> Vec<(String, String)> {
    request
        .lines()
        .skip(1)
        .take_while(|line| !line.trim().is_empty())
        .filter_map(|line| {
            let (name, value) = line.split_once(':')?;
            Some((name.trim().to_ascii_lowercase(), value.trim().to_string()))
        })
        .collect()
}

fn is_authorized(headers: &[(String, String)], config: &ServerConfig) -> bool {
    let username = headers
        .iter()
        .find(|(name, _)| name == "x-appmanager-username")
        .map(|(_, value)| value.as_str())
        .unwrap_or_default();
    let password = headers
        .iter()
        .find(|(name, _)| name == "x-appmanager-password")
        .map(|(_, value)| value.as_str())
        .unwrap_or_default();

    username == config.username
        && ((!config.password.is_empty() && password == config.password)
            || (!config.password_hash.is_empty() && make_id(password) == config.password_hash))
}

fn write_json_response(stream: &mut TcpStream, status: u16, body: &str) {
    let status_text = match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        500 => "Internal Server Error",
        _ => "OK",
    };

    let response = format!(
        "HTTP/1.1 {status} {status_text}\r\nContent-Type: application/json; charset=utf-8\r\nAccess-Control-Allow-Origin: *\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    let _ = stream.write_all(response.as_bytes());
}

#[allow(clippy::too_many_arguments)]
fn emit_transfer_progress(
    app_handle: Option<&tauri::AppHandle>,
    direction: &str,
    app_id: &str,
    app_name: &str,
    transferred: u64,
    total: u64,
    _started_at: Instant,
    status: &str,
) {
    if let Some(app_handle) = app_handle {
        let key = transfer_key(direction, app_id);
        let speed = calculate_transfer_speed(&key, transferred, status);
        let percent = if total == 0 {
            0.0
        } else {
            (transferred as f64 / total as f64 * 100.0).min(100.0)
        };
        let progress = TransferProgress {
            app_id: app_id.to_string(),
            app_name: app_name.to_string(),
            direction: direction.to_string(),
            transferred,
            total,
            speed,
            percent,
            status: status.to_string(),
        };
        if let Ok(mut store) = transfer_progress_store().lock() {
            store.insert(key, progress.clone());
        }
        let result = app_handle.emit("transfer-progress", progress);
        if let Err(error) = result {
            log_debug(&format!(
                "emit transfer failed direction={} app_id={} status={} error={}",
                direction, app_id, status, error
            ));
        }
    }
}

fn calculate_transfer_speed(key: &str, transferred: u64, status: &str) -> u64 {
    let now_ms = now_millis();
    let Ok(mut samples) = transfer_speed_samples().lock() else {
        return 0;
    };

    if status != "running" {
        samples.insert(
            key.to_string(),
            TransferSpeedSample {
                transferred,
                timestamp_ms: now_ms,
                speed: 0,
            },
        );
        return 0;
    }

    let speed = match samples.get(key) {
        Some(previous) if transferred >= previous.transferred && now_ms > previous.timestamp_ms => {
            let delta_bytes = transferred - previous.transferred;
            let delta_ms = now_ms - previous.timestamp_ms;
            (delta_bytes as u128 * 1000)
                .checked_div(delta_ms)
                .map(|value| value as u64)
                .unwrap_or(previous.speed)
        }
        Some(previous) => previous.speed,
        None => 0,
    };

    samples.insert(
        key.to_string(),
        TransferSpeedSample {
            transferred,
            timestamp_ms: now_ms,
            speed,
        },
    );
    speed
}

fn client_get(config: &ClientConfig, path: &str) -> Result<HttpResponse, String> {
    client_get_streaming(config, path, "download", path, "远程软件", None)
}

fn client_get_status(config: &ClientConfig, path: &str) -> Result<HttpResponse, String> {
    client_get_with_timeouts(
        config,
        path,
        Duration::from_millis(CLIENT_STATUS_CONNECT_TIMEOUT_MS),
        Duration::from_millis(CLIENT_STATUS_IO_TIMEOUT_MS),
    )
}

fn client_get_with_timeouts(
    config: &ClientConfig,
    path: &str,
    connect_timeout: Duration,
    io_timeout: Duration,
) -> Result<HttpResponse, String> {
    if config.host.trim().is_empty() {
        return Err("请先填写服务端地址".to_string());
    }

    if config.username.trim().is_empty() || config.password.is_empty() {
        return Err("请先填写客户端用户名和密码".to_string());
    }

    let address = format!("{}:{}", config.host.trim(), config.port);
    let socket_addr = address
        .to_socket_addrs()
        .map_err(|error| format!("解析服务端地址失败：{error}"))?
        .next()
        .ok_or_else(|| "解析服务端地址失败：没有可用地址".to_string())?;
    let mut stream = TcpStream::connect_timeout(&socket_addr, connect_timeout)
        .map_err(|error| format!("连接服务端失败：{error}"))?;
    let _ = stream.set_read_timeout(Some(io_timeout));
    let _ = stream.set_write_timeout(Some(io_timeout));

    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: {}:{}\r\nX-AppManager-Username: {}\r\nX-AppManager-Password: {}\r\nConnection: close\r\n\r\n",
        config.host.trim(),
        config.port,
        config.username.trim(),
        config.password
    );
    stream
        .write_all(request.as_bytes())
        .map_err(error_message)?;

    let mut response = Vec::new();
    stream.read_to_end(&mut response).map_err(error_message)?;
    parse_http_response(response)
}

fn client_connection_status(config: &ClientConfig) -> ClientConnectionStatus {
    let configured = !config.host.trim().is_empty()
        && !config.username.trim().is_empty()
        && !config.password.is_empty();
    let mut status = ClientConnectionStatus {
        configured,
        online: false,
        host: config.host.trim().to_string(),
        port: config.port,
        username: config.username.trim().to_string(),
        message: if configured {
            "未检测".to_string()
        } else {
            "请先填写服务端地址、用户名和密码".to_string()
        },
        server_name: None,
        server_mode: None,
        allow_downloads: None,
        checked_at: now(),
    };

    if !configured {
        return status;
    }

    match client_get_status(config, "/api/auth/test") {
        Ok(response) if response.status == 200 => {
            status.online = true;
            status.message = "连接正常".to_string();
            if let Ok(info_response) = client_get_status(config, "/api/server-info") {
                let value = serde_json::from_slice::<serde_json::Value>(&info_response.body).ok();
                status.server_name = value
                    .as_ref()
                    .and_then(|item| item.get("name"))
                    .and_then(|item| item.as_str())
                    .map(|item| item.to_string());
                status.server_mode = value
                    .as_ref()
                    .and_then(|item| item.get("mode"))
                    .and_then(|item| item.as_str())
                    .map(|item| item.to_string());
                status.allow_downloads = value
                    .as_ref()
                    .and_then(|item| item.get("allowDownloads"))
                    .and_then(|item| item.as_bool());
            }
        }
        Ok(response) if response.status == 401 => {
            status.message = "认证失败，请检查用户名和密码".to_string();
        }
        Ok(response) => {
            status.message = format!("连接异常，HTTP 状态：{}", response.status);
        }
        Err(error) => {
            status.message = error;
        }
    }

    status
}

fn inactive_client_connection_status(settings: &Settings) -> ClientConnectionStatus {
    ClientConnectionStatus {
        configured: false,
        online: false,
        host: settings.client.host.trim().to_string(),
        port: settings.client.port,
        username: settings.client.username.trim().to_string(),
        message: match settings.run_mode.as_str() {
            "server" => "当前为服务端模式，客户端连接功能未启用".to_string(),
            "local" => "当前为本地模式，远程连接功能未启用".to_string(),
            _ => "客户端连接功能未启用".to_string(),
        },
        server_name: None,
        server_mode: None,
        allow_downloads: None,
        checked_at: now(),
    }
}

fn ensure_client_mode(settings: &Settings) -> Result<(), String> {
    if settings.run_mode == "client" {
        Ok(())
    } else {
        Err("当前不是客户端模式，远程客户端功能未启用".to_string())
    }
}

fn client_get_streaming(
    config: &ClientConfig,
    path: &str,
    direction: &str,
    app_id: &str,
    app_name: &str,
    app_handle: Option<&tauri::AppHandle>,
) -> Result<HttpResponse, String> {
    if config.host.trim().is_empty() {
        return Err("请先填写服务端地址".to_string());
    }

    if config.username.trim().is_empty() || config.password.is_empty() {
        return Err("请先填写客户端用户名和密码".to_string());
    }

    let mut stream = TcpStream::connect(format!("{}:{}", config.host.trim(), config.port))
        .map_err(|error| format!("连接服务端失败：{error}"))?;
    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: {}:{}\r\nX-AppManager-Username: {}\r\nX-AppManager-Password: {}\r\nConnection: close\r\n\r\n",
        config.host.trim(),
        config.port,
        config.username.trim(),
        config.password
    );
    stream
        .write_all(request.as_bytes())
        .map_err(error_message)?;

    let mut response = Vec::new();
    let mut headers_complete = false;
    let mut header_end = 0usize;
    let mut total = 0u64;
    let mut transferred = 0u64;
    let started_at = Instant::now();
    let mut last_emit = Instant::now();
    let mut chunk = [0u8; 64 * 1024];

    loop {
        let count = stream.read(&mut chunk).map_err(error_message)?;
        if count == 0 {
            break;
        }
        response.extend_from_slice(&chunk[..count]);

        if !headers_complete {
            if let Some(position) = response.windows(4).position(|window| window == b"\r\n\r\n") {
                headers_complete = true;
                header_end = position + 4;
                let header_text = String::from_utf8_lossy(&response[..position]);
                let headers = header_text
                    .lines()
                    .skip(1)
                    .filter_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        Some((name.trim().to_ascii_lowercase(), value.trim().to_string()))
                    })
                    .collect::<Vec<_>>();
                total = header_value(&headers, "content-length")
                    .and_then(|value| value.parse::<u64>().ok())
                    .unwrap_or(0);
            }
        }

        if headers_complete {
            transferred = response.len().saturating_sub(header_end) as u64;
            if last_emit.elapsed().as_millis() > 350 || transferred == total {
                emit_transfer_progress(
                    app_handle,
                    direction,
                    app_id,
                    app_name,
                    transferred,
                    total,
                    started_at,
                    "running",
                );
                last_emit = Instant::now();
            }
        }
    }

    if !headers_complete {
        return Err("服务端响应格式无效，未收到完整响应头".to_string());
    }

    if total > 0 && transferred != total {
        emit_transfer_progress(
            app_handle,
            direction,
            app_id,
            app_name,
            transferred,
            total,
            started_at,
            "error",
        );
        return Err(format!(
            "下载未完成：已接收 {} / {} 字节",
            transferred, total
        ));
    }

    if headers_complete {
        emit_transfer_progress(
            app_handle,
            direction,
            app_id,
            app_name,
            transferred,
            total,
            started_at,
            "running",
        );
    }

    parse_http_response(response)
}

fn client_get_to_file(
    config: &ClientConfig,
    path: &str,
    direction: &str,
    app_id: &str,
    app_name: &str,
    app_handle: Option<&tauri::AppHandle>,
    target_path: &Path,
) -> Result<HttpFileResponse, String> {
    if config.host.trim().is_empty() {
        return Err("请先填写服务端地址".to_string());
    }

    if config.username.trim().is_empty() || config.password.is_empty() {
        return Err("请先填写客户端用户名和密码".to_string());
    }

    if let Some(parent) = target_path.parent() {
        fs::create_dir_all(parent).map_err(error_message)?;
    }
    if target_path.exists() {
        let _ = fs::remove_file(target_path);
    }

    log_debug(&format!(
        "client file download start host={} port={} path={} target={}",
        config.host.trim(),
        config.port,
        path,
        native_path_to_string(target_path)
    ));
    let mut stream = TcpStream::connect(format!("{}:{}", config.host.trim(), config.port))
        .map_err(|error| format!("杩炴帴鏈嶅姟绔け璐ワ細{error}"))?;
    let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: {}:{}\r\nX-AppManager-Username: {}\r\nX-AppManager-Password: {}\r\nConnection: close\r\n\r\n",
        config.host.trim(),
        config.port,
        config.username.trim(),
        config.password
    );
    stream
        .write_all(request.as_bytes())
        .map_err(error_message)?;

    let started_at = Instant::now();
    let mut last_emit = Instant::now();
    let mut buffer = Vec::with_capacity(64 * 1024);
    let mut chunk = [0u8; 64 * 1024];
    let mut headers_complete = false;
    let mut status = 0u16;
    let mut headers = Vec::new();
    let mut total = 0u64;
    let mut transferred = 0u64;
    let mut output: Option<fs::File> = None;
    let mut chunk_index = 0u64;
    let mut next_progress_log = 10 * 1024 * 1024u64;
    let mut next_emit_bytes = 2 * 1024 * 1024u64;
    let mut last_pack_poll = Instant::now()
        .checked_sub(Duration::from_millis(800))
        .unwrap_or_else(Instant::now);

    loop {
        let count = match stream.read(&mut chunk) {
            Ok(count) => count,
            Err(error)
                if !headers_complete
                    && matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
            {
                if last_pack_poll.elapsed().as_millis() >= 500 {
                    poll_remote_pack_progress(
                        config, path, direction, app_id, app_name, app_handle,
                    );
                    last_pack_poll = Instant::now();
                }
                continue;
            }
            Err(error) => {
                log_debug(&format!(
                    "client file download read failed path={} transferred={} total={} error={}",
                    path, transferred, total, error
                ));
                return Err(error_message(error));
            }
        };
        if count == 0 {
            log_debug(&format!(
                "client file download eof path={} transferred={} total={} headers_complete={}",
                path, transferred, total, headers_complete
            ));
            break;
        }
        chunk_index += 1;

        if !headers_complete {
            buffer.extend_from_slice(&chunk[..count]);
            if let Some(position) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
                headers_complete = true;
                let header_text = String::from_utf8_lossy(&buffer[..position]);
                let mut lines = header_text.lines();
                let status_line = lines.next().ok_or_else(|| "服务端响应为空".to_string())?;
                status = status_line
                    .split_whitespace()
                    .nth(1)
                    .and_then(|value| value.parse::<u16>().ok())
                    .ok_or_else(|| "无法读取服务端状态码".to_string())?;
                headers = lines
                    .filter_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        Some((name.trim().to_ascii_lowercase(), value.trim().to_string()))
                    })
                    .collect::<Vec<_>>();
                total = header_value(&headers, "content-length")
                    .and_then(|value| value.parse::<u64>().ok())
                    .unwrap_or(0);
                log_debug(&format!(
                    "client file download headers path={} status={} total={} buffered={} body_start={}",
                    path,
                    status,
                    total,
                    buffer.len(),
                    position + 4
                ));
                let _ = stream.set_read_timeout(None);
                poll_remote_pack_progress(config, path, direction, app_id, app_name, app_handle);

                let mut file = fs::File::create(target_path).map_err(error_message)?;
                let body_start = position + 4;
                if body_start < buffer.len() {
                    file.write_all(&buffer[body_start..])
                        .map_err(error_message)?;
                    transferred += (buffer.len() - body_start) as u64;
                }
                output = Some(file);
                buffer.clear();
                emit_transfer_progress(
                    app_handle,
                    direction,
                    app_id,
                    app_name,
                    transferred,
                    total,
                    started_at,
                    "running",
                );
                last_emit = Instant::now();
            }
        } else if let Some(file) = output.as_mut() {
            file.write_all(&chunk[..count]).map_err(error_message)?;
            transferred += count as u64;
        }

        if headers_complete
            && (chunk_index <= 3 || transferred >= next_progress_log || transferred == total)
        {
            log_debug(&format!(
                "client file download progress path={} chunk={} transferred={} total={}",
                path, chunk_index, transferred, total
            ));
            while transferred >= next_progress_log {
                next_progress_log += 10 * 1024 * 1024;
            }
        }

        if headers_complete
            && (chunk_index <= 3
                || transferred >= next_emit_bytes
                || last_emit.elapsed().as_millis() > 350
                || transferred == total)
        {
            emit_transfer_progress(
                app_handle,
                direction,
                app_id,
                app_name,
                transferred,
                total,
                started_at,
                "running",
            );
            last_emit = Instant::now();
            while transferred >= next_emit_bytes {
                next_emit_bytes += 512 * 1024;
            }
        }
    }

    if !headers_complete {
        let _ = fs::remove_file(target_path);
        log_debug(&format!(
            "client file download failed no headers path={} buffered={}",
            path,
            buffer.len()
        ));
        return Err("服务端响应格式无效，未收到完整响应头".to_string());
    }

    drop(output);

    if total > 0 && transferred != total {
        emit_transfer_progress(
            app_handle,
            direction,
            app_id,
            app_name,
            transferred,
            total,
            started_at,
            "error",
        );
        let _ = fs::remove_file(target_path);
        log_debug(&format!(
            "client file download incomplete path={} transferred={} total={} target={}",
            path,
            transferred,
            total,
            native_path_to_string(target_path)
        ));
        return Err(format!(
            "下载未完成：已接收 {} / {} 字节",
            transferred, total
        ));
    }

    emit_transfer_progress(
        app_handle,
        direction,
        app_id,
        app_name,
        transferred,
        total,
        started_at,
        "running",
    );
    log_debug(&format!(
        "client file download done path={} status={} transferred={} total={} target={}",
        path,
        status,
        transferred,
        total,
        native_path_to_string(target_path)
    ));

    Ok(HttpFileResponse {
        status,
        headers,
        body_path: target_path.to_path_buf(),
        transferred,
        total,
    })
}

fn poll_remote_pack_progress(
    config: &ClientConfig,
    download_path: &str,
    direction: &str,
    app_id: &str,
    app_name: &str,
    app_handle: Option<&tauri::AppHandle>,
) {
    let Some(progress_path) = download_path
        .strip_prefix("/api/apps/")
        .and_then(|value| value.strip_suffix("/download"))
        .map(|id| format!("/api/apps/{id}/progress"))
    else {
        return;
    };

    match client_get(config, &progress_path) {
        Ok(response) if response.status == 200 => {
            if let Ok(Some(progress)) =
                serde_json::from_slice::<Option<TransferProgress>>(&response.body)
            {
                if progress.status == "packing" || progress.percent >= 100.0 {
                    emit_transfer_progress(
                        app_handle,
                        direction,
                        app_id,
                        app_name,
                        progress.transferred,
                        progress.total,
                        Instant::now(),
                        "packing",
                    );
                }
            } else {
                log_debug(&format!(
                    "client pack progress poll parse empty path={} body_len={}",
                    progress_path,
                    response.body.len()
                ));
            }
        }
        Ok(response) => {
            log_debug(&format!(
                "client pack progress poll status={} path={}",
                response.status, progress_path
            ));
        }
        Err(error) => {
            log_debug(&format!(
                "client pack progress poll failed path={} error={}",
                progress_path, error
            ));
        }
    }
}

fn client_upload(
    config: &ClientConfig,
    app: &ManagedApp,
    upload_path: &Path,
    file_name: &str,
    app_handle: &tauri::AppHandle,
) -> Result<String, String> {
    if config.host.trim().is_empty() {
        return Err("请先填写服务端地址".to_string());
    }

    if config.username.trim().is_empty() || config.password.is_empty() {
        return Err("请先填写客户端用户名和密码".to_string());
    }

    let mut file = fs::File::open(upload_path).map_err(error_message)?;
    let total = file.metadata().map_err(error_message)?.len();
    let mut stream = TcpStream::connect(format!("{}:{}", config.host.trim(), config.port))
        .map_err(|error| format!("杩炴帴鏈嶅姟绔け璐ワ細{error}"))?;
    let request = format!(
        "POST /api/apps/upload HTTP/1.1\r\nHost: {}:{}\r\nX-AppManager-Username: {}\r\nX-AppManager-Password: {}\r\nX-AppManager-Filename: {}\r\nX-AppManager-Category: {}\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        config.host.trim(),
        config.port,
        config.username.trim(),
        config.password,
        file_name,
        app.category_name,
        total
    );
    stream
        .write_all(request.as_bytes())
        .map_err(error_message)?;

    let started_at = Instant::now();
    let mut transferred = 0u64;
    let mut last_emit = Instant::now();
    let mut chunk = [0u8; 64 * 1024];
    loop {
        let count = file.read(&mut chunk).map_err(error_message)?;
        if count == 0 {
            break;
        }
        stream.write_all(&chunk[..count]).map_err(error_message)?;
        transferred += count as u64;
        if last_emit.elapsed().as_millis() > 350 || transferred == total {
            emit_transfer_progress(
                Some(app_handle),
                "upload",
                &app.id,
                &app.name,
                transferred,
                total,
                started_at,
                "running",
            );
            last_emit = Instant::now();
        }
    }
    let _ = stream.shutdown(Shutdown::Write);
    emit_transfer_progress(
        Some(app_handle),
        "upload",
        &app.id,
        &app.name,
        transferred,
        total,
        started_at,
        "done",
    );

    let mut response = Vec::new();
    stream.read_to_end(&mut response).map_err(error_message)?;
    let response = parse_http_response(response)?;
    if response.status == 200 {
        Ok("上传完成，已进入服务端未审核软件".to_string())
    } else if response.status == 401 {
        Err("认证失败，请检查用户名和密码".to_string())
    } else {
        Err(format!(
            "上传失败，HTTP 状态：{}{}",
            response.status,
            response_error_message(&response)
        ))
    }
}

fn parse_http_response(response: Vec<u8>) -> Result<HttpResponse, String> {
    let split_at = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| "服务端响应格式无效".to_string())?;
    let header_bytes = &response[..split_at];
    let body = response[split_at + 4..].to_vec();
    let header_text = String::from_utf8_lossy(header_bytes);
    let mut lines = header_text.lines();
    let status_line = lines.next().ok_or_else(|| "服务端响应为空".to_string())?;
    let status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|value| value.parse::<u16>().ok())
        .ok_or_else(|| "无法读取服务端状态码".to_string())?;
    Ok(HttpResponse { status, body })
}

fn response_error_message(response: &HttpResponse) -> String {
    serde_json::from_slice::<serde_json::Value>(&response.body)
        .ok()
        .and_then(|value| {
            value
                .get("error")
                .and_then(|error| error.as_str())
                .map(|error| format!("：{error}"))
        })
        .unwrap_or_default()
}

fn response_error_message_from_file(path: &Path) -> String {
    fs::File::open(path)
        .ok()
        .and_then(|file| {
            let mut body = Vec::new();
            file.take(16 * 1024).read_to_end(&mut body).ok()?;
            serde_json::from_slice::<serde_json::Value>(&body).ok()
        })
        .and_then(|value| {
            value
                .get("error")
                .and_then(|error| error.as_str())
                .map(|error| format!("：{error}"))
        })
        .unwrap_or_default()
}

fn header_value(headers: &[(String, String)], name: &str) -> Option<String> {
    let name = name.to_ascii_lowercase();
    headers
        .iter()
        .find(|(header_name, _)| header_name == &name)
        .map(|(_, value)| value.clone())
}

fn list_review_items(library_path: &Path) -> Result<Vec<ReviewItem>, String> {
    let review_path = review_dir_path(library_path);
    fs::create_dir_all(&review_path).map_err(error_message)?;
    let mut items = Vec::new();

    for entry in fs::read_dir(&review_path).map_err(error_message)? {
        let entry = entry.map_err(error_message)?;
        let path = entry.path();
        if !path.is_file() || is_review_metadata_file(&path) {
            continue;
        }
        let file_name = path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("未命名软件")
            .to_string();
        let meta_path = review_meta_path(&path);
        let metadata = fs::read_to_string(meta_path)
            .ok()
            .and_then(|content| serde_json::from_str::<serde_json::Value>(&content).ok());
        let category_name = metadata
            .as_ref()
            .and_then(|value| value.get("categoryName"))
            .and_then(|value| value.as_str())
            .unwrap_or("来自客户端")
            .to_string();
        let uploaded_at = metadata
            .as_ref()
            .and_then(|value| value.get("uploadedAt"))
            .and_then(|value| value.as_u64())
            .unwrap_or(0);
        let extracted_paths = metadata
            .as_ref()
            .and_then(|value| value.get("extractedPaths"))
            .and_then(|value| value.as_array())
            .map(|values| {
                values
                    .iter()
                    .filter_map(|value| value.as_str().map(|value| value.to_string()))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let name = path
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or(&file_name)
            .to_string();
        items.push(ReviewItem {
            id: make_id(&path_to_string(&path)),
            name,
            file_name,
            category_name,
            size: entry.metadata().map_err(error_message)?.len(),
            uploaded_at,
            path: path_to_string(&path),
            extracted_paths,
        });
    }

    items.sort_by_key(|item| Reverse(item.uploaded_at));
    Ok(items)
}

fn is_review_metadata_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.ends_with(".meta.json"))
}

fn review_meta_path(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("upload");
    path.with_file_name(format!("{file_name}.meta.json"))
}

fn review_dir_path(library_path: &Path) -> PathBuf {
    let ascii_path = library_path.join(APPS_DIR).join(REVIEW_FOLDER);
    if ascii_path.exists() {
        return ascii_path;
    }

    let legacy_path = library_path.join(APPS_DIR).join(REVIEW_DIR);
    if legacy_path.exists() {
        return legacy_path;
    }

    ascii_path
}

fn category_path_for_name(library_path: &Path, category_name: &str) -> PathBuf {
    library_path
        .join(APPS_DIR)
        .join(category_folder_name(category_name))
}

fn category_storage_path(library_path: &Path, category_name: &str) -> PathBuf {
    if category_name == REVIEW_DIR {
        review_dir_path(library_path)
    } else {
        category_path_for_name(library_path, category_name)
    }
}

fn category_folder_name(category_name: &str) -> String {
    category_slug(category_name)
}

fn category_slug(category_name: &str) -> String {
    let trimmed = strip_sort_prefix(category_name).trim().to_lowercase();
    let mut parts = Vec::new();
    let mut ascii = String::new();
    for char in trimmed.chars() {
        if char.is_ascii_alphanumeric() {
            ascii.push(char.to_ascii_lowercase());
            continue;
        }

        if !ascii.is_empty() {
            parts.push(std::mem::take(&mut ascii));
        }

        if let Some(pinyin) = char.to_pinyin() {
            parts.push(pinyin.plain().to_string());
        } else if is_cjk_char(char) {
            parts.push(format!("u{:x}", char as u32));
        }
    }

    if !ascii.is_empty() {
        parts.push(ascii);
    }

    let slug = parts.join("-");
    if slug.is_empty() {
        "category".to_string()
    } else {
        slug
    }
}

fn is_cjk_char(char: char) -> bool {
    matches!(
        char as u32,
        0x4E00..=0x9FFF | 0x3400..=0x4DBF | 0x20000..=0x2A6DF | 0x2A700..=0x2B73F
    )
}

fn cleanup_review_extracted_paths(library_path: &Path, paths: &[String]) -> Result<(), String> {
    for value in paths {
        let path = PathBuf::from(normalize_incoming_path(value));
        ensure_inside_review_dir(library_path, &path)?;
        if path.is_dir() {
            fs::remove_dir_all(&path).map_err(error_message)?;
        } else if path.is_file() {
            fs::remove_file(&path).map_err(error_message)?;
        }
    }
    Ok(())
}

fn remove_review_apps_by_paths(data: &mut AppData, paths: &[String]) {
    if paths.is_empty() {
        return;
    }
    let normalized = paths
        .iter()
        .map(|path| normalize_incoming_path(path))
        .collect::<HashSet<_>>();
    data.apps
        .retain(|item| !normalized.contains(&normalize_incoming_path(&item.folder_path)));
}

fn unique_file_name(folder: &Path, file_name: &str) -> String {
    let candidate = sanitize_file_name(file_name);
    if !folder.join(&candidate).exists() {
        return candidate;
    }

    let path = Path::new(&candidate);
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("app");
    let extension = path.extension().and_then(|value| value.to_str());
    for index in 1..1000 {
        let next = if let Some(extension) = extension {
            format!("{stem}-{index}.{extension}")
        } else {
            format!("{stem}-{index}")
        };
        if !folder.join(&next).exists() {
            return next;
        }
    }

    format!("{}-{}", now(), candidate)
}

fn ensure_category_by_name(
    library_path: &Path,
    data: &mut AppData,
    category_name: &str,
) -> Result<Category, String> {
    let clean_name = validate_category_name(category_name)?;
    if let Some(index) = data
        .categories
        .iter()
        .position(|item| item.name == clean_name)
    {
        if clean_name == REVIEW_DIR {
            let review_path = review_dir_path(library_path);
            let review_path_text = path_to_string(&review_path);
            if data.categories[index].path != review_path_text {
                fs::create_dir_all(&review_path).map_err(error_message)?;
                data.categories[index].path = review_path_text;
                data.categories[index].updated_at = now();
            }
        }
        return Ok(data.categories[index].clone());
    }

    let now = now();
    let category_path = category_storage_path(library_path, &clean_name);
    fs::create_dir_all(&category_path).map_err(error_message)?;
    let category = Category {
        id: make_id(&clean_name),
        name: clean_name,
        path: path_to_string(&category_path),
        created_at: now,
        updated_at: now,
    };
    data.categories.push(category.clone());
    Ok(category)
}

fn expand_zip_to_category(zip_path: &Path, category_path: &Path) -> Result<(), String> {
    fs::create_dir_all(category_path).map_err(error_message)?;
    let file = fs::File::open(zip_path).map_err(error_message)?;
    let mut archive = ZipArchive::new(file).map_err(error_message)?;
    log_debug(&format!(
        "zip extract start source={} target={} entries={}",
        native_path_to_string(zip_path),
        native_path_to_string(category_path),
        archive.len()
    ));

    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(error_message)?;
        let Some(enclosed_name) = entry.enclosed_name().map(|path| path.to_path_buf()) else {
            log_debug(&format!(
                "zip extract skip unsafe entry source={} index={} name={}",
                native_path_to_string(zip_path),
                index,
                entry.name()
            ));
            continue;
        };
        let output_path = category_path.join(enclosed_name);
        if entry.is_dir() {
            fs::create_dir_all(&output_path).map_err(error_message)?;
            continue;
        }
        if let Some(parent) = output_path.parent() {
            fs::create_dir_all(parent).map_err(error_message)?;
        }
        let mut output = fs::File::create(&output_path).map_err(error_message)?;
        std::io::copy(&mut entry, &mut output).map_err(error_message)?;
    }

    log_debug(&format!(
        "zip extract done source={} target={}",
        native_path_to_string(zip_path),
        native_path_to_string(category_path)
    ));
    Ok(())
}

fn create_stored_zip(
    source_path: &Path,
    zip_path: &Path,
    progress: Option<ZipProgress<'_>>,
    source_signature: Option<&PackageSourceSignature>,
) -> Result<(), String> {
    if let Some(parent) = zip_path.parent() {
        fs::create_dir_all(parent).map_err(error_message)?;
    }
    log_debug(&format!(
        "zip create start source={} target={}",
        native_path_to_string(source_path),
        native_path_to_string(zip_path)
    ));

    let base_parent = source_path
        .parent()
        .ok_or_else(|| "无法读取软件文件夹名".to_string())?;
    let files = match source_signature {
        Some(signature) => signature.files.clone(),
        None => package_source_signature(source_path)?.files,
    };
    let file_count = files.len();
    let total_bytes = source_signature
        .map(|signature| signature.total_bytes)
        .unwrap_or_else(|| files.iter().map(|file| file.len).sum());
    log_debug(&format!(
        "zip create collected source={} files={} total_bytes={}",
        native_path_to_string(source_path),
        file_count,
        total_bytes
    ));
    let progress = progress.as_ref();
    if let Some(progress) = progress {
        log_debug(&format!(
            "zip progress emit start direction={} app_id={} status={} transferred=0 total={}",
            progress.direction, progress.app_id, progress.status, total_bytes
        ));
        emit_transfer_progress(
            progress.app_handle,
            progress.direction,
            progress.app_id,
            progress.app_name,
            0,
            total_bytes,
            Instant::now(),
            progress.status,
        );
    }

    let zip_file = fs::File::create(zip_path).map_err(error_message)?;
    let mut zip = ZipWriter::new(zip_file);
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    let mut buffer = vec![0u8; ZIP_BUFFER_SIZE];
    let started_at = Instant::now();
    let mut last_emit = Instant::now();
    let mut packed_bytes = 0u64;

    for file in files {
        let entry_name = zip_entry_name(base_parent, &file.path)?;
        zip.start_file(entry_name, options).map_err(error_message)?;

        let mut input = fs::File::open(&file.path).map_err(error_message)?;
        loop {
            let count = input.read(&mut buffer).map_err(error_message)?;
            if count == 0 {
                break;
            }
            zip.write_all(&buffer[..count]).map_err(error_message)?;
            packed_bytes = packed_bytes.saturating_add(count as u64);
            emit_zip_progress_if_needed(
                progress,
                packed_bytes,
                total_bytes,
                total_bytes,
                started_at,
                &mut last_emit,
                false,
            );
        }
    }
    emit_zip_progress_if_needed(
        progress,
        total_bytes,
        total_bytes,
        total_bytes,
        started_at,
        &mut last_emit,
        true,
    );

    let zip_file = zip.finish().map_err(error_message)?;
    let zip_size = zip_file
        .metadata()
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    log_debug(&format!(
        "zip create done target={} files={} size={}",
        native_path_to_string(zip_path),
        file_count,
        zip_size
    ));
    Ok(())
}

#[derive(Debug, Clone)]
struct PackageSourceFile {
    path: PathBuf,
    len: u64,
    modified_ms: u128,
}

#[derive(Debug, Clone)]
struct PackageSourceSignature {
    files: Vec<PackageSourceFile>,
    total_bytes: u64,
    signature_hash: u64,
}

fn package_source_signature(path: &Path) -> Result<PackageSourceSignature, String> {
    let mut files = Vec::new();
    collect_package_source_files(path, &mut files)?;
    files.sort_by(|left, right| left.path.cmp(&right.path));

    let total_bytes = files
        .iter()
        .fold(0u64, |total, file| total.saturating_add(file.len));
    let signature_hash = package_source_hash(&files);

    Ok(PackageSourceSignature {
        files,
        total_bytes,
        signature_hash,
    })
}

fn collect_package_source_files(
    path: &Path,
    files: &mut Vec<PackageSourceFile>,
) -> Result<(), String> {
    if path.is_file() {
        files.push(package_source_file(path)?);
        return Ok(());
    }

    for entry in fs::read_dir(path).map_err(error_message)? {
        let entry = entry.map_err(error_message)?;
        let entry_path = entry.path();
        let file_type = entry.file_type().map_err(error_message)?;
        if file_type.is_dir() {
            collect_package_source_files(&entry_path, files)?;
        } else if file_type.is_file() {
            files.push(package_source_file(&entry_path)?);
        }
    }

    Ok(())
}

fn package_source_file(path: &Path) -> Result<PackageSourceFile, String> {
    let metadata = fs::metadata(path).map_err(error_message)?;
    let modified_ms = metadata
        .modified()
        .ok()
        .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
        .map(|value| value.as_millis())
        .unwrap_or(0);
    Ok(PackageSourceFile {
        path: path.to_path_buf(),
        len: metadata.len(),
        modified_ms,
    })
}

fn package_source_hash(files: &[PackageSourceFile]) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for file in files {
        update_package_hash(&mut hash, native_path_to_string(&file.path).as_bytes());
        update_package_hash(&mut hash, &file.len.to_le_bytes());
        update_package_hash(&mut hash, &file.modified_ms.to_le_bytes());
    }
    hash
}

fn update_package_hash(hash: &mut u64, bytes: &[u8]) {
    for byte in bytes {
        *hash ^= u64::from(*byte);
        *hash = hash.wrapping_mul(0x100000001b3);
    }
}

fn package_cache_path(
    library_path: &Path,
    app_id: &str,
    signature: &PackageSourceSignature,
    file_name: &str,
) -> Result<PathBuf, String> {
    let cache_dir = package_cache_dir(library_path);
    let safe_app_id = sanitize_file_name(app_id);
    let safe_file_name = sanitize_file_name(file_name);
    let cache_key = format!(
        "{}-{}-{:016x}",
        signature.files.len(),
        signature.total_bytes,
        signature.signature_hash
    );
    Ok(cache_dir.join(format!("{safe_app_id}-{cache_key}-{safe_file_name}")))
}

fn package_cache_dir(library_path: &Path) -> PathBuf {
    library_path.join(CONFIG_DIR).join(PACKAGE_CACHE_DIR)
}

fn package_cache_info(library_path: &Path) -> Result<PackageCacheInfo, String> {
    let cache_dir = package_cache_dir(library_path);
    fs::create_dir_all(&cache_dir).map_err(error_message)?;
    let mut file_count = 0u64;
    let mut total_size = 0u64;

    for entry in fs::read_dir(&cache_dir).map_err(error_message)? {
        let entry = entry.map_err(error_message)?;
        let metadata = entry.metadata().map_err(error_message)?;
        if metadata.is_file() {
            file_count = file_count.saturating_add(1);
            total_size = total_size.saturating_add(metadata.len());
        }
    }

    Ok(PackageCacheInfo {
        path: path_to_string(&cache_dir),
        file_count,
        total_size,
    })
}

fn cleanup_package_cache_for_app(library_path: &Path, app_id: &str) {
    let cache_dir = package_cache_dir(library_path);
    let safe_app_id = sanitize_file_name(app_id);
    let Ok(entries) = fs::read_dir(cache_dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let Some(file_name) = path.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        if file_name.starts_with(&format!("{safe_app_id}-")) {
            let _ = fs::remove_file(path);
        }
    }
}

fn zip_entry_name(base_parent: &Path, file_path: &Path) -> Result<String, String> {
    let relative = file_path.strip_prefix(base_parent).map_err(error_message)?;
    let name = relative
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/");
    if name.is_empty() {
        Err("无法生成压缩包内文件名".to_string())
    } else {
        Ok(name)
    }
}

fn emit_zip_progress_if_needed(
    progress: Option<&ZipProgress<'_>>,
    work_done: u64,
    total_work: u64,
    display_total: u64,
    started_at: Instant,
    last_emit: &mut Instant,
    force: bool,
) {
    let Some(progress) = progress else {
        return;
    };
    if !force && last_emit.elapsed().as_millis() <= 350 && work_done < total_work {
        return;
    }
    let transferred = if total_work == 0 {
        0
    } else {
        ((work_done as u128).saturating_mul(display_total as u128) / total_work as u128)
            .min(display_total as u128) as u64
    };
    emit_transfer_progress(
        progress.app_handle,
        progress.direction,
        progress.app_id,
        progress.app_name,
        transferred,
        display_total,
        started_at,
        progress.status,
    );
    *last_emit = Instant::now();
}

fn snapshot_directory_entries(folder: &Path) -> Result<HashSet<String>, String> {
    if !folder.exists() {
        return Ok(HashSet::new());
    }

    fs::read_dir(folder)
        .map_err(error_message)?
        .map(|entry| {
            entry
                .map_err(error_message)
                .map(|entry| path_to_string(&entry.path()))
        })
        .collect()
}

fn new_directory_entries(folder: &Path, before: &HashSet<String>) -> Result<Vec<PathBuf>, String> {
    let mut paths = fs::read_dir(folder)
        .map_err(error_message)?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| !before.contains(&path_to_string(path)))
        .collect::<Vec<_>>();
    paths.sort();
    Ok(paths)
}

fn sanitize_file_name(value: &str) -> String {
    let invalid_chars = ['<', '>', ':', '"', '/', '\\', '|', '?', '*'];
    let cleaned = value
        .chars()
        .map(|char| {
            if invalid_chars.contains(&char) {
                '_'
            } else {
                char
            }
        })
        .collect::<String>()
        .trim()
        .to_string();

    if cleaned.is_empty() {
        "app".to_string()
    } else {
        cleaned
    }
}

fn normalize_incoming_path(path: &str) -> String {
    if cfg!(windows) {
        path.replace('/', "\\")
    } else {
        path.to_string()
    }
}

fn scan_categories(
    library_path: &Path,
    data: &mut AppData,
    categories: &[Category],
) -> Result<ScanResult, String> {
    let mut added = 0;
    let mut updated = 0;
    let mut issues = Vec::new();

    for category in categories {
        let category_path = PathBuf::from(&category.path);
        ensure_inside_apps_dir(library_path, &category_path)?;
        fs::create_dir_all(&category_path).map_err(error_message)?;

        for entry in fs::read_dir(&category_path).map_err(error_message)? {
            let entry = entry.map_err(error_message)?;
            let file_type = entry.file_type().map_err(error_message)?;

            if file_type.is_dir() {
                let folder_path = entry.path();
                let app_name = entry.file_name().to_string_lossy().trim().to_string();
                if app_name.is_empty() {
                    continue;
                }

                let executables = find_executables(&folder_path)?;
                let executable_path = pick_executable(&app_name, &folder_path, &executables);
                if executables.is_empty() {
                    issues.push(ScanIssue {
                        folder_path: path_to_string(&folder_path),
                        reason: "未找到 .exe 文件".to_string(),
                        candidates: Vec::new(),
                    });
                } else if executables.len() > 1 && executable_path.is_none() {
                    issues.push(ScanIssue {
                        folder_path: path_to_string(&folder_path),
                        reason: "检测到多个 .exe，需要后续选择主程序".to_string(),
                        candidates: executables
                            .iter()
                            .map(|item| path_to_string(item))
                            .collect(),
                    });
                }

                if upsert_scanned_app(
                    library_path,
                    data,
                    category,
                    &app_name,
                    &folder_path,
                    executable_path,
                    &executables,
                ) {
                    updated += 1;
                } else {
                    added += 1;
                }
            } else if file_type.is_file() {
                let executable_path = entry.path();
                if !is_executable_file(&executable_path) {
                    continue;
                }

                let app_name = executable_path
                    .file_stem()
                    .and_then(|value| value.to_str())
                    .unwrap_or("未命名软件")
                    .trim()
                    .to_string();

                if upsert_scanned_app(
                    library_path,
                    data,
                    category,
                    &app_name,
                    &category_path,
                    Some(executable_path),
                    &[],
                ) {
                    updated += 1;
                } else {
                    added += 1;
                }
            }
        }
    }

    Ok(ScanResult {
        added,
        updated,
        issues,
        data: data.clone(),
    })
}

fn scan_app_paths(
    library_path: &Path,
    data: &mut AppData,
    category: &Category,
    paths: &[PathBuf],
) -> Result<ScanResult, String> {
    let mut added = 0;
    let mut updated = 0;
    let mut issues = Vec::new();

    for path in paths {
        if !path.exists() {
            continue;
        }
        let Some((was_updated, issue)) = scan_app_path(library_path, data, category, path)? else {
            continue;
        };
        if was_updated {
            updated += 1;
        } else {
            added += 1;
        }
        if let Some(issue) = issue {
            issues.push(issue);
        }
    }

    Ok(ScanResult {
        added,
        updated,
        issues,
        data: data.clone(),
    })
}

fn scan_app_path(
    library_path: &Path,
    data: &mut AppData,
    category: &Category,
    path: &Path,
) -> Result<Option<(bool, Option<ScanIssue>)>, String> {
    ensure_inside_apps_dir(library_path, path)?;
    if path.is_dir() {
        let app_name = path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("未命名软件")
            .trim()
            .to_string();
        if app_name.is_empty() {
            return Ok(None);
        }

        let executables = find_executables(path)?;
        let executable_path = pick_executable(&app_name, path, &executables);
        let issue = if executables.is_empty() {
            Some(ScanIssue {
                folder_path: path_to_string(path),
                reason: "未找到 .exe 文件".to_string(),
                candidates: Vec::new(),
            })
        } else if executables.len() > 1 && executable_path.is_none() {
            Some(ScanIssue {
                folder_path: path_to_string(path),
                reason: "检测到多个 .exe，需要后续选择主程序".to_string(),
                candidates: executables
                    .iter()
                    .map(|item| path_to_string(item))
                    .collect(),
            })
        } else {
            None
        };

        let was_updated = upsert_scanned_app(
            library_path,
            data,
            category,
            &app_name,
            path,
            executable_path,
            &executables,
        );
        Ok(Some((was_updated, issue)))
    } else if path.is_file() && is_executable_file(path) {
        let app_name = path
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("未命名软件")
            .trim()
            .to_string();
        let folder_path = path.parent().unwrap_or(path);
        let was_updated = upsert_scanned_app(
            library_path,
            data,
            category,
            &app_name,
            folder_path,
            Some(path.to_path_buf()),
            &[],
        );
        Ok(Some((was_updated, None)))
    } else {
        Ok(None)
    }
}

fn upsert_scanned_app(
    _library_path: &Path,
    data: &mut AppData,
    category: &Category,
    app_name: &str,
    folder_path: &Path,
    executable_path: Option<PathBuf>,
    executable_candidates: &[PathBuf],
) -> bool {
    let app_id = make_id(&path_to_string(
        executable_path.as_deref().unwrap_or(folder_path),
    ));
    let icon_data_url = None;

    let folder_key = path_to_string(folder_path);
    let existing_index = data
        .apps
        .iter()
        .position(|item| item.id == app_id)
        .or_else(|| {
            executable_path.as_ref().and_then(|_| {
                data.apps.iter().position(|item| {
                    item.executable_path.is_none()
                        && normalize_incoming_path(&item.folder_path)
                            == normalize_incoming_path(&folder_key)
                })
            })
        });

    if let Some(index) = existing_index {
        let app = &mut data.apps[index];
        app.id = app_id;
        app.name = app_name.to_string();
        app.category_id = category.id.clone();
        app.category_name = category.name.clone();
        app.folder_path = folder_key;
        app.executable_path = executable_path.as_ref().map(|item| path_to_string(item));
        app.executable_candidates =
            normalize_executable_candidates(executable_path.as_ref(), executable_candidates);
        app.icon_data_url = icon_data_url.or(app.icon_data_url.take());
        true
    } else {
        data.apps.push(ManagedApp {
            id: app_id,
            name: app_name.to_string(),
            category_id: category.id.clone(),
            category_name: category.name.clone(),
            folder_path: folder_key,
            executable_path: executable_path.as_ref().map(|item| path_to_string(item)),
            executable_candidates: normalize_executable_candidates(
                executable_path.as_ref(),
                executable_candidates,
            ),
            icon_data_url,
            favorite: false,
            note: String::new(),
            launch_count: 0,
            last_launched_at: None,
        });
        false
    }
}

fn find_executables(folder: &Path) -> Result<Vec<PathBuf>, String> {
    let mut executables = Vec::new();
    collect_executables(folder, &mut executables, 0)?;
    executables.sort();
    Ok(executables)
}

fn normalize_executable_candidates(
    executable_path: Option<&PathBuf>,
    executable_candidates: &[PathBuf],
) -> Vec<String> {
    let mut values = executable_candidates
        .iter()
        .map(|path| path_to_string(path))
        .collect::<Vec<_>>();
    if let Some(executable_path) = executable_path {
        let executable_path = path_to_string(executable_path);
        if !values.iter().any(|value| value == &executable_path) {
            values.insert(0, executable_path);
        }
    }
    values.sort();
    values.dedup();
    values
}

fn is_executable_file(path: &Path) -> bool {
    path.extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case("exe"))
}

fn base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = String::with_capacity(bytes.len().div_ceil(3) * 4);

    for chunk in bytes.chunks(3) {
        let first = chunk[0];
        let second = *chunk.get(1).unwrap_or(&0);
        let third = *chunk.get(2).unwrap_or(&0);

        output.push(TABLE[(first >> 2) as usize] as char);
        output.push(TABLE[(((first & 0b0000_0011) << 4) | (second >> 4)) as usize] as char);

        if chunk.len() > 1 {
            output.push(TABLE[(((second & 0b0000_1111) << 2) | (third >> 6)) as usize] as char);
        } else {
            output.push('=');
        }

        if chunk.len() > 2 {
            output.push(TABLE[(third & 0b0011_1111) as usize] as char);
        } else {
            output.push('=');
        }
    }

    output
}

fn base64_decode(value: &str) -> Result<Vec<u8>, String> {
    let clean_value = value.trim();
    if clean_value.is_empty() || clean_value.len() % 4 != 0 {
        return Err("图标图片内容无效".to_string());
    }

    let mut output = Vec::with_capacity(clean_value.len() / 4 * 3);
    let bytes = clean_value.as_bytes();
    let chunk_count = bytes.len() / 4;
    for (index, chunk) in bytes.chunks(4).enumerate() {
        let is_last = index + 1 == chunk_count;
        let pad = chunk.iter().rev().take_while(|byte| **byte == b'=').count();
        if pad > 2 || (!is_last && pad > 0) {
            return Err("图标图片内容无效".to_string());
        }

        let mut value24 = 0u32;
        for (offset, byte) in chunk.iter().enumerate() {
            let six_bits = if *byte == b'=' {
                if !is_last || offset < 2 {
                    return Err("图标图片内容无效".to_string());
                }
                0
            } else {
                base64_value(*byte).ok_or_else(|| "图标图片内容无效".to_string())?
            };
            value24 = (value24 << 6) | u32::from(six_bits);
        }

        output.push(((value24 >> 16) & 0xff) as u8);
        if pad < 2 {
            output.push(((value24 >> 8) & 0xff) as u8);
        }
        if pad < 1 {
            output.push((value24 & 0xff) as u8);
        }
    }

    Ok(output)
}

fn base64_value(byte: u8) -> Option<u8> {
    match byte {
        b'A'..=b'Z' => Some(byte - b'A'),
        b'a'..=b'z' => Some(byte - b'a' + 26),
        b'0'..=b'9' => Some(byte - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

fn collect_executables(
    folder: &Path,
    executables: &mut Vec<PathBuf>,
    depth: usize,
) -> Result<(), String> {
    if depth > 3 {
        return Ok(());
    }

    for entry in fs::read_dir(folder).map_err(error_message)? {
        let entry = entry.map_err(error_message)?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(error_message)?;

        if file_type.is_dir() {
            collect_executables(&path, executables, depth + 1)?;
        } else if is_executable_file(&path) {
            executables.push(path);
        }
    }

    Ok(())
}

fn pick_executable(app_name: &str, app_folder: &Path, executables: &[PathBuf]) -> Option<PathBuf> {
    if executables.len() == 1 {
        return executables.first().cloned();
    }

    let normalized_name = normalize_name(app_name);
    let normalized_clean_name = normalize_name(&strip_sort_prefix(app_name));

    for expected_name in [&normalized_name, &normalized_clean_name] {
        if expected_name.is_empty() {
            continue;
        }
        if let Some(path) = executables
            .iter()
            .find(|path| executable_stem_matches(path, expected_name))
        {
            return Some(path.clone());
        }
    }

    if let Some(path) = executables.iter().find(|path| {
        path.parent().is_some_and(|parent| parent == app_folder)
            && path
                .file_stem()
                .and_then(|value| value.to_str())
                .is_some_and(|stem| {
                    let stem = normalize_name(&strip_sort_prefix(stem));
                    !stem.is_empty() && normalized_clean_name.contains(&stem)
                        || !normalized_clean_name.is_empty()
                            && stem.contains(&normalized_clean_name)
                })
    }) {
        return Some(path.clone());
    }

    executables
        .iter()
        .find(|path| path.parent().is_some_and(|parent| parent == app_folder))
        .cloned()
}

fn executable_stem_matches(path: &Path, expected_name: &str) -> bool {
    path.file_stem()
        .and_then(|value| value.to_str())
        .map(|stem| normalize_name(&strip_sort_prefix(stem)))
        .is_some_and(|stem| stem == expected_name)
}

fn load_or_create_data(library_path: &Path) -> Result<AppData, String> {
    let data_path = data_path(library_path);
    if !data_path.exists() {
        let data = AppData {
            library_path: path_to_string(library_path),
            categories: Vec::new(),
            apps: Vec::new(),
            settings: Settings::default(),
        };
        save_data(library_path, &data)?;
        return Ok(data);
    }

    let content = fs::read_to_string(&data_path).map_err(error_message)?;
    let mut data: AppData = serde_json::from_str(&content).map_err(error_message)?;
    hydrate_icon_data_urls(&mut data);
    Ok(data)
}

fn save_data(library_path: &Path, data: &AppData) -> Result<(), String> {
    let config_path = library_path.join(CONFIG_DIR);
    fs::create_dir_all(config_path).map_err(error_message)?;
    let mut data_to_save = data.clone();
    persist_icon_references(library_path, &mut data_to_save)?;
    let content = serde_json::to_string_pretty(&data_to_save).map_err(error_message)?;
    fs::write(data_path(library_path), content).map_err(error_message)
}

fn persist_icon_references(library_path: &Path, data: &mut AppData) -> Result<(), String> {
    for app in &mut data.apps {
        let Some(icon_value) = app.icon_data_url.as_deref() else {
            continue;
        };
        if !icon_value.starts_with("data:image/") {
            continue;
        }

        let folder_path = PathBuf::from(normalize_incoming_path(&app.folder_path));
        ensure_inside_apps_dir(library_path, &folder_path)?;
        fs::create_dir_all(&folder_path).map_err(error_message)?;

        let (mime, bytes) = decode_image_data_url(icon_value)?;
        let extension = icon_extension_from_mime(mime)?;
        remove_existing_app_icon_files(&folder_path)?;
        let icon_path = folder_path.join(format!("{APP_ICON_FILE_STEM}.{extension}"));
        fs::write(&icon_path, bytes).map_err(error_message)?;
        app.icon_data_url = Some(path_to_string(&icon_path));
    }
    Ok(())
}

fn hydrate_icon_data_urls(data: &mut AppData) {
    for app in &mut data.apps {
        let Some(icon_value) = app.icon_data_url.as_deref() else {
            continue;
        };
        if icon_value.starts_with("data:image/") {
            continue;
        }

        app.icon_data_url = read_image_as_data_url(icon_value).ok();
    }
}

fn remove_existing_app_icon_files(folder_path: &Path) -> Result<(), String> {
    let extensions = ["png", "jpg", "gif", "webp", "ico", "bmp"];
    for extension in extensions {
        let path = folder_path.join(format!("{APP_ICON_FILE_STEM}.{extension}"));
        if path.exists() {
            fs::remove_file(path).map_err(error_message)?;
        }
    }
    Ok(())
}

fn library_root() -> Result<PathBuf, String> {
    let exe_path = std::env::current_exe().map_err(error_message)?;
    let app_dir = exe_path
        .parent()
        .ok_or_else(|| "鏃犳硶鑾峰彇 APP 鐩綍".to_string())?;
    Ok(app_dir.join(LIBRARY_DIR))
}

fn data_path(library_path: &Path) -> PathBuf {
    library_path.join(CONFIG_DIR).join(DATA_FILE)
}

fn validate_category_name(name: &str) -> Result<String, String> {
    let clean_name = name.trim();
    if clean_name.is_empty() {
        return Err("鍒嗙被鍚嶇О涓嶈兘涓虹┖".to_string());
    }

    let invalid_chars = ['<', '>', ':', '"', '/', '\\', '|', '?', '*'];
    if clean_name.chars().any(|char| invalid_chars.contains(&char)) {
        return Err("分类名称包含 Windows 文件夹非法字符".to_string());
    }

    Ok(clean_name.to_string())
}

fn ensure_inside_apps_dir(library_path: &Path, target_path: &Path) -> Result<(), String> {
    let apps_path = library_path.join(APPS_DIR);
    let target = target_path
        .canonicalize()
        .unwrap_or_else(|_| target_path.to_path_buf());
    let apps = apps_path.canonicalize().unwrap_or(apps_path);

    if !target.starts_with(&apps) {
        return Err("目标路径不在软件库 Apps 目录内，已取消操作".to_string());
    }

    Ok(())
}

fn ensure_inside_review_dir(library_path: &Path, target_path: &Path) -> Result<(), String> {
    let review_path = review_dir_path(library_path);
    let target = target_path
        .canonicalize()
        .unwrap_or_else(|_| target_path.to_path_buf());
    let review = review_path.canonicalize().unwrap_or(review_path);

    if !target.starts_with(&review) {
        return Err("鐩爣璺緞涓嶅湪鏈鏍歌蒋浠剁洰褰曞唴锛屽凡鍙栨秷鎿嶄綔".to_string());
    }

    Ok(())
}

fn hidden_command(program: impl AsRef<OsStr>) -> Command {
    let mut command = Command::new(program);
    #[cfg(windows)]
    {
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

fn make_id(value: &str) -> String {
    let mut hash = 14695981039346656037u64;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(1099511628211);
    }
    format!("{hash:x}")
}

fn normalize_name(value: &str) -> String {
    value
        .chars()
        .filter(|char| char.is_ascii_alphanumeric())
        .collect::<String>()
        .to_lowercase()
}

fn strip_sort_prefix(value: &str) -> String {
    let trimmed = value.trim();
    let bytes = trimmed.as_bytes();
    let mut index = 0;

    while index < bytes.len() && bytes[index].is_ascii_digit() {
        index += 1;
    }

    if index > 0 && index < bytes.len() && matches!(bytes[index], b'.' | b'-' | b'_' | b' ') {
        trimmed[index + 1..].trim().to_string()
    } else {
        trimmed.to_string()
    }
}

fn path_to_string(path: &Path) -> String {
    native_path_to_string(path)
}

fn native_path_to_string(path: &Path) -> String {
    path.to_string_lossy().to_string()
}

fn log_debug(message: &str) {
    if !ENABLE_DEBUG_LOGS {
        return;
    }
    let Ok(library_path) = library_root() else {
        return;
    };
    let config_path = library_path.join(CONFIG_DIR);
    if fs::create_dir_all(&config_path).is_err() {
        return;
    }
    let log_path = config_path.join("appmanager-debug.log");
    let Ok(mut file) = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)
    else {
        return;
    };
    let _ = writeln!(file, "[{}] {}", now_millis(), message);
}

fn next_temp_file_sequence() -> u64 {
    let sequence = TEMP_FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_millis() as u64)
        .unwrap_or(0);
    (millis << 16) ^ sequence
}

fn now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_millis())
        .unwrap_or(0)
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs())
        .unwrap_or(0)
}

fn error_message(error: impl std::fmt::Display) -> String {
    error.to_string()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            init_library,
            create_category,
            delete_category,
            scan_category,
            scan_all,
            toggle_favorite,
            delete_app,
            move_app_to_category,
            update_app_info,
            update_settings,
            get_server_status,
            get_package_cache_info,
            get_client_connection_status,
            update_favorite_order,
            clear_package_cache,
            get_transfer_progress,
            debug_log,
            test_client_connection,
            fetch_remote_apps,
            list_review_apps,
            approve_review_app,
            reject_review_app,
            download_remote_app,
            upload_app_to_server,
            launch_app,
            launch_app_as_admin,
            reveal_path
        ])
        .run(tauri::generate_context!())
        .expect("error while running AppManager");
}
