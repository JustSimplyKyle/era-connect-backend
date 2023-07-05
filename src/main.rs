use anyhow::{Context, Result};
use futures::{io::empty, stream::FuturesUnordered, Stream, StreamExt};
use reqwest::Url;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        atomic::Ordering,
        atomic::{AtomicBool, AtomicUsize},
        Arc,
    },
    thread::JoinHandle,
    time::Duration,
};
use tokio::{
    fs::{self, File},
    io::AsyncWriteExt,
    time::{self, Instant},
};

#[derive(Debug, PartialEq, Serialize, Deserialize)]
enum ActionType {
    #[serde(rename = "allow")]
    Allow,
    #[serde(rename = "disallow")]
    Disallow,
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
enum OsName {
    #[serde(rename = "osx")]
    Osx,
    #[serde(rename = "windows")]
    Windows,
    #[serde(rename = "linux")]
    Linux,
}
#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct Os {
    name: Option<OsName>,
    version: Option<String>,
    arch: Option<String>,
}
#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct Rule {
    action: ActionType,
    features: Option<HashMap<String, bool>>,
    os: Option<Os>,
    value: Option<Vec<String>>,
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct GameFlags {
    rules: Vec<Rule>,
    arguments: Vec<String>,
    additional_arguments: Option<Vec<String>>,
}
#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct JvmFlags {
    rules: Vec<Rule>,
    arguments: Vec<String>,
    additional_arguments: Option<Vec<String>>,
}
#[derive(Debug, PartialEq, Serialize, Deserialize, Default)]
struct AssetIndex {
    id: String,
    sha1: String,
    size: usize,
    #[serde(rename = "totalSize")]
    total_size: usize,
    url: String,
}

#[derive(Debug, PartialEq, Serialize, Deserialize, Default)]
struct DownloadMetadata {
    sha1: String,
    size: usize,
    url: String,
}

#[derive(Debug, PartialEq, Serialize, Deserialize, Default)]
struct Downloads {
    client: DownloadMetadata,
    client_mappings: DownloadMetadata,
    server: DownloadMetadata,
    server_mappings: DownloadMetadata,
}
#[derive(Debug, PartialEq, Serialize, Deserialize, Default)]
struct LoggingConfig {
    client: ClientConfig,
}

#[derive(Debug, PartialEq, Serialize, Deserialize, Default)]
struct ClientConfig {
    argument: String,
    file: LogFile,
    #[serde(rename = "type")]
    log_type: String,
}

#[derive(Debug, PartialEq, Serialize, Deserialize, Default)]
struct LogFile {
    id: String,
    sha1: String,
    size: usize,
    url: String,
}
#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct LibraryMetadata {
    path: String,
    sha1: String,
    size: usize,
    url: String,
}
#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct LibraryArtifact {
    artifact: LibraryMetadata,
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct Library {
    downloads: LibraryArtifact,
    name: String,
    rules: Option<Vec<Rule>>,
}
fn get_rules(argument: &mut [Value]) -> Result<Vec<Rule>> {
    let rules: Result<Vec<Rule>, _> = argument
        .iter_mut()
        .filter(|x| x["rules"][0].is_object())
        .map(|x| serde_json::from_value(x["rules"][0].take()))
        .collect();

    rules.context("Failed to collect/serialize rules")
}

struct JvmArgs {
    launcher_name: String,
    launcher_version: String,
    classpath: String,
    classpath_separator: String,
    primary_jar: String,
    library_directory: String,
    game_directory: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let target = "https://piston-meta.mojang.com/v1/packages/715ccf3330885e75b205124f09f8712542cbe7e0/1.20.1.json";
    let response = reqwest::get(target).await?;
    let mut contents: Value = response.json().await?;

    let game_argument = contents["arguments"]["game"].as_array_mut().unwrap();

    let game_flags = GameFlags {
        rules: get_rules(game_argument)?,
        arguments: game_argument
            .iter()
            .filter_map(Value::as_str)
            .map(std::string::ToString::to_string)
            .collect::<Vec<_>>(),
        additional_arguments: None,
    };

    let asset_index: AssetIndex = serde_json::from_value(contents["assetIndex"].take())
        .context("Failed to Serialize assetIndex")?;

    let downloads_list: Downloads = serde_json::from_value(contents["downloads"].take())
        .context("Failed to Serialize Downloads")?;

    let mut library_list: Vec<Library> = serde_json::from_value(contents["libraries"].take())?;

    let logging: LoggingConfig = serde_json::from_value(contents["logging"].take())
        .context("Failed to Serialize logging")?;

    let main_class: String =
        serde_json::from_value(contents["mainClass"].take()).context("Failed to get MainClass")?;

    let client_jar = extract_filename(&downloads_list.client.url).unwrap();

    let mut classpath_list = library_list
        .iter()
        .map(|x| x.downloads.artifact.path.to_string())
        .collect::<Vec<String>>();

    classpath_list.push(client_jar.to_string());
    let classpath = classpath_list.join(":");

    let jvm_argument = contents["arguments"]["jvm"].as_array_mut().unwrap();
    let jvm_flags = JvmFlags {
        rules: get_rules(jvm_argument)?,
        arguments: jvm_argument
            .iter()
            .filter_map(Value::as_str)
            .map(std::string::ToString::to_string)
            .collect::<Vec<_>>(),
        additional_arguments: None,
    };
    let jvm_options = JvmArgs {
        launcher_name: "era-connect".to_string(),
        launcher_version: "0.0.1".to_string(),
        classpath,
        classpath_separator: ":".to_string(),
        primary_jar: client_jar.to_string(),
        library_directory: "./".to_string(),
        game_directory: "~/.minecraft".to_string(),
    };
    parallel_library(library_list).await?;
    // let list_future = download_file(downloads_list.client.url, None);
    // let asset_future = download_file(asset_index.url, Some(generate_unique_filename()));
    // let test_future = download_file(downloads_list.server.url, None);
    // let _ = join!(list_future, asset_future, test_future);
    Ok(())
}

pub async fn parallel_join(
    handles: &mut FuturesUnordered<tokio::task::JoinHandle<std::result::Result<(), anyhow::Error>>>,
    size_counter: &Arc<AtomicUsize>,
) -> Result<()> {
    let total_items = handles.len();
    let start_time = Instant::now();
    while let Some(handle) = handles.next().await {
        let total_bytes = Arc::clone(&size_counter);
        handle.unwrap()?;
        let current_bytes = total_bytes.load(Ordering::SeqCst);
        let elapsed_time = start_time.elapsed().as_secs_f64();
        dbg!(current_bytes, elapsed_time);
        let speed = current_bytes as f64 / elapsed_time / 1_000_000.0;

        let progress: f64 = 1.0 - (handles.len() as f64 / total_items as f64);
        println!(
            "{:.5}% [{}/{}] item processed | Average Speed: {:.2} MB/s",
            progress * 100.0,
            total_items - handles.len(),
            total_items,
            speed
        );
        // time::sleep(Duration::from_millis(10)).await;
    }
    let total_bytes = Arc::clone(&size_counter);
    let elapsed_time = start_time.elapsed().as_secs_f64();
    let overall_speed = total_bytes.load(Ordering::SeqCst) as f64 / elapsed_time / 1_000_000.0;
    println!("Overall Speed: {:.2} MB/s", overall_speed);
    Ok(())
}
pub async fn parallel_library(library_list: Vec<Library>) -> Result<()> {
    let library_list_arc: Arc<Vec<Library>> = Arc::new(library_list);
    let counter = Arc::new(AtomicUsize::new(0));
    let size_counter = Arc::new(AtomicUsize::new(0));
    let num_libraries = library_list_arc.len();

    let mut library_download_handles = FuturesUnordered::new();
    let push = Arc::new(AtomicBool::new(true));
    for _ in 0..num_libraries {
        let library_list_clone = Arc::clone(&library_list_arc);
        let counter_clone = Arc::clone(&counter);
        let size_clone = Arc::clone(&size_counter);
        let push_clone = Arc::clone(&push);
        let handle = tokio::spawn(async move {
            let index = counter_clone.fetch_add(1, Ordering::SeqCst);
            if index < num_libraries {
                let library = &library_list_clone[index];
                if Path::new(&library.downloads.artifact.path).exists() {
                    push_clone.store(false, Ordering::Release);
                    test(format!("{} already downloaded!", library.name)).await
                } else {
                    download_library(library, size_clone).await
                }
            } else {
                Ok(())
            }
        });
        // "race condition"
        time::sleep(Duration::from_millis(1)).await;
        let push_clone = Arc::clone(&push);
        if push_clone.load(Ordering::Acquire) {
            library_download_handles.push(handle);
        }
        push_clone.store(true, Ordering::SeqCst);
    }
    parallel_join(&mut library_download_handles, &size_counter).await?;

    Ok(())
}
async fn test(msg: String) -> Result<()> {
    println!("{msg}");
    std::future::ready("").await;
    Ok(())
}
async fn download_file(
    url: String,
    name: Option<String>,
    total_bytes: Arc<AtomicUsize>,
) -> Result<()> {
    let mut response = reqwest::get(&url).await?;
    let filename = if let Some(x) = name {
        x.to_string()
    } else {
        extract_filename(&url).unwrap()
    };
    let mut file = File::create(&filename).await?;
    while let Some(chunk) = response.chunk().await? {
        file.write_all(&chunk).await?;
        total_bytes.fetch_add(chunk.len() as usize, Ordering::Relaxed);
    }
    Ok(())
}

async fn download_library(library: &Library, total_bytes: Arc<AtomicUsize>) -> Result<()> {
    let path = library.downloads.artifact.path.to_string();
    let parent_dir = std::path::Path::new(&path).parent().unwrap();
    fs::create_dir_all(parent_dir).await?;
    let url = library.downloads.artifact.url.to_string();
    download_file(url, Some(path), total_bytes).await?;
    Ok(())
}

fn generate_unique_filename() -> String {
    // Generate a unique filename, such as using a timestamp or a random identifier
    // For simplicity, this example uses a timestamp-based filename
    let timestamp = chrono::Utc::now().timestamp();
    format!("file_{}.txt", timestamp)
}

fn extract_filename(url: &String) -> Result<String, Box<dyn std::error::Error>> {
    let parsed_url = Url::parse(url)?;
    let path_segments = parsed_url.path_segments().ok_or("Invalid URL")?;
    let filename = path_segments.last().ok_or("No filename found in URL")?;
    Ok(filename.to_string())
}
