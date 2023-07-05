use anyhow::{Context, Result};
use reqwest::Url;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{cell::RefCell, collections::HashMap, rc::Rc};
use tokio::{
    fs::{self, File},
    io::AsyncWriteExt,
    join, spawn,
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
struct Library {
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

    let asset_index: AssetIndex = serde_json::from_value(contents["assetIndex"].take())
        .context("Failed to Serialize assetIndex")?;

    let downloads_list: Downloads = serde_json::from_value(contents["downloads"].take())
        .context("Failed to Serialize Downloads")?;
    let library_list: Vec<Library> = serde_json::from_value(contents["libraries"].take())?;

    let logging: LoggingConfig = serde_json::from_value(contents["logging"].take())
        .context("Failed to Serialize logging")?;

    let main_class: String =
        serde_json::from_value(contents["mainClass"].take()).context("Failed to get MainClass")?;

    use futures::stream::{FuturesUnordered, StreamExt};
    use tokio::task;

    let library_list_clone = library_list.leak();
    let mut library_download_handles = FuturesUnordered::new();
    for x in library_list_clone.iter() {
        let handle = task::spawn(async move { download_library(&x).await });
        library_download_handles.push(handle);
    }
    let total_file_downloads = library_download_handles.len();
    while let Some(handle) = library_download_handles.next().await {
        handle??;
        let progress: f64 =
            1.0 - (library_download_handles.len() as f64 / total_file_downloads as f64);
        println!(
            "{:.5}% [{}/{}] file processed",
            progress * 100.0,
            total_file_downloads - library_download_handles.len(),
            total_file_downloads
        );
    }
    // let list_future = download_file(downloads_list.client.url, None);
    // let asset_future = download_file(asset_index.url, Some(generate_unique_filename()));
    // let test_future = download_file(downloads_list.server.url, None);
    // let _ = join!(list_future, asset_future, test_future);
    Ok(())
}

async fn download_file(url: String, name: Option<String>) -> Result<()> {
    let mut response = reqwest::get(&url).await?;
    let filename = if let Some(x) = name {
        x.to_string()
    } else {
        extract_filename(&url).unwrap()
    };
    let mut file = File::create(&filename).await?;
    while let Some(chunk) = response.chunk().await? {
        file.write_all(&chunk).await?;
    }
    Ok(())
}
async fn download_library(library: &Library) -> Result<()> {
    let path = library.downloads.artifact.path.to_string();
    let parent_dir = std::path::Path::new(&path).parent().unwrap();
    fs::create_dir_all(parent_dir).await?;
    let url = library.downloads.artifact.url.to_string();
    download_file(url, Some(path)).await?;
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
