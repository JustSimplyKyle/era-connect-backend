use anyhow::{Context, Result};
use async_semaphore::Semaphore;
use futures::{stream::FuturesUnordered, StreamExt};
use reqwest::Url;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        atomic::Ordering,
        atomic::{AtomicBool, AtomicUsize},
        Arc,
    },
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

#[derive(Debug, PartialEq, Serialize, Deserialize, Clone, Copy)]
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

    let current_size = Arc::new(AtomicUsize::new(0));
    let library_path = Arc::new(PathBuf::from("library"));
    let (mut handles, total_size) =
        parallel_library(library_list, library_path, Arc::clone(&current_size)).await;

    let client_jar_future =
        download_file(downloads_list.client.url, None, Arc::clone(&current_size));
    handles.push(tokio::spawn(async move { client_jar_future.await }));
    total_size.fetch_add(downloads_list.client.size, Ordering::Relaxed);

    let (asset_download_list, asset_download_path, asset_download_size) =
        asset_extraction(asset_index).await?;

    let max_concurrent_tasks = 200;
    let semaphore = Arc::new(Semaphore::new(max_concurrent_tasks));

    let asset_download_list_arc = Arc::new(asset_download_list);
    let asset_download_path_arc = Arc::new(asset_download_path);
    let asset_download_size_arc = Arc::new(asset_download_size);

    parallel_assets(
        asset_download_list_arc,
        asset_download_path_arc,
        asset_download_size_arc,
        &semaphore,
        &current_size,
        &total_size,
        &mut handles,
    )
    .await?;

    let download_complete = Arc::new(AtomicBool::new(false));
    let download_complete_clone = Arc::clone(&download_complete);
    let current_size_clone = Arc::clone(&current_size);
    let instant = Instant::now();
    let task = tokio::spawn(async move {
        while !download_complete_clone.load(Ordering::Acquire) {
            time::sleep(Duration::from_millis(10)).await;
            // dbg!(current_size_clone.load(Ordering::Relaxed) as f64 / total_size as f64);
            println!(
                "{:.2}%, {:.2} MiBs, {}/{}",
                current_size_clone.load(Ordering::Relaxed) as f64
                    / total_size.load(Ordering::Relaxed) as f64
                    * 100.0,
                current_size_clone.load(Ordering::Relaxed) as f64
                    / instant.elapsed().as_secs_f64()
                    / 1_000_000.0,
                current_size_clone.load(Ordering::Relaxed),
                total_size.load(Ordering::Relaxed)
            );
            // time::sleep(Duration::from_millis(15)).await;
        }
        download_complete_clone.store(true, Ordering::SeqCst);
        // std::future::ready(Ok(())).await
    });

    parallel_join(&mut handles).await?;

    // I'm not sure completely how this works... but it certainly does!
    download_complete.store(true, Ordering::Release);
    task.await?;
    Ok(())
}

async fn parallel_assets(
    asset_download_list: Arc<Vec<String>>,
    asset_download_path: Arc<Vec<PathBuf>>,
    asset_download_size: Arc<Vec<usize>>,
    semaphore: &Arc<Semaphore>,
    current_size: &Arc<AtomicUsize>,
    total_size: &Arc<AtomicUsize>,
    handles: &mut FuturesUnordered<tokio::task::JoinHandle<std::result::Result<(), anyhow::Error>>>,
) -> Result<()> {
    for index in 0..asset_download_list.len() {
        let asset_download_list_clone = Arc::clone(&asset_download_list);
        let asset_download_path_clone = Arc::clone(&asset_download_path);
        let semaphore_clone = Arc::clone(&semaphore);
        let current_size_clone = Arc::clone(&current_size);
        fs::create_dir_all(asset_download_path_clone[index].parent().unwrap()).await?;
        if asset_download_path_clone[index].exists() {
            println!(
                "skip downloading asset {}",
                asset_download_list_clone[index]
            );
        } else {
            total_size.fetch_add(asset_download_size[index], Ordering::Relaxed);
            handles.push(tokio::spawn(async move {
                let _per = semaphore_clone.acquire().await;
                download_file(
                    asset_download_list_clone[index].to_string(),
                    // I *could* fix this, but I'm lazy
                    Some(asset_download_path_clone[index].clone()),
                    current_size_clone,
                )
                .await
            }));
        }
    }
    Ok(())
}

async fn asset_extraction(
    asset_index: AssetIndex,
) -> Result<(Vec<String>, Vec<PathBuf>, Vec<usize>)> {
    let asset_response = reqwest::get(asset_index.url).await?;
    let asset_index_content: Value = asset_response.json().await?;
    let asset_objects = asset_index_content["objects"].as_object().unwrap();
    let mut asset_download_list = Vec::new();
    let mut asset_download_path = Vec::new();
    let mut asset_download_size: Vec<usize> = Vec::new();
    for (_, val) in asset_objects {
        asset_download_list.push(format!(
            "https://resources.download.minecraft.net/{:.2}/{}",
            val["hash"].as_str().unwrap(),
            val["hash"].as_str().unwrap()
        ));
        asset_download_path.push(PathBuf::from(format!(
            ".minecraft/assets/objects/{:.2}/{}",
            val["hash"].as_str().unwrap(),
            val["hash"].as_str().unwrap()
        )));
        asset_download_size.push(val["size"].as_u64().unwrap().try_into().unwrap());
    }
    Ok((
        asset_download_list,
        asset_download_path,
        asset_download_size,
    ))
}

pub async fn parallel_join(
    handles: &mut FuturesUnordered<tokio::task::JoinHandle<std::result::Result<(), anyhow::Error>>>,
    // size_counter: &Arc<AtomicUsize>,
    // size_sum: usize,
) -> Result<()> {
    while let Some(handle) = handles.next().await {
        handle??;
    }
    Ok(())
}
pub async fn parallel_library(
    library_list: Vec<Library>,
    folder: Arc<PathBuf>,
    current: Arc<AtomicUsize>,
) -> (
    FuturesUnordered<tokio::task::JoinHandle<std::result::Result<(), anyhow::Error>>>,
    Arc<AtomicUsize>,
) {
    let library_list_arc: Arc<Vec<Library>> = Arc::new(library_list);
    let index_counter = Arc::new(AtomicUsize::new(0));
    let index_spawn_counter = Arc::new(AtomicUsize::new(0));
    let size_counter = current;
    let download_total_size = Arc::new(AtomicUsize::new(0));
    let do_library_os = Arc::new(AtomicBool::new(false));
    let num_libraries = library_list_arc.len();

    let library_download_handles = FuturesUnordered::new();
    let push = Arc::new(AtomicBool::new(true));
    let current_os = os_version::detect().unwrap();
    let current_os_type = match current_os {
        os_version::OsVersion::Linux(_) => OsName::Linux,
        os_version::OsVersion::Windows(_) => OsName::Windows,
        os_version::OsVersion::MacOS(_) => OsName::Osx,
        _ => panic!("not supported"),
    };
    for _ in 0..num_libraries {
        let library_list_clone = Arc::clone(&library_list_arc);
        let counter_clone = Arc::clone(&index_counter);
        let counter2_clone = Arc::clone(&index_spawn_counter);
        let size_clone = Arc::clone(&size_counter);
        let push_clone = Arc::clone(&push);
        let folder = Arc::clone(&folder);
        let do_library_clone = Arc::clone(&do_library_os);
        let index = counter2_clone.fetch_add(1, Ordering::SeqCst);
        if index < num_libraries {
            let library = &library_list_clone[index];
            let final_path = folder.join(&library.downloads.artifact.path);
            if !final_path.exists() || do_library_clone.load(Ordering::SeqCst) {
                download_total_size.fetch_add(library.downloads.artifact.size, Ordering::SeqCst);
            }
        }
        let handle = tokio::spawn(async move {
            let index = counter_clone.fetch_add(1, Ordering::SeqCst);
            if index < num_libraries {
                let library = &library_list_clone[index];
                let final_path = folder.join(&library.downloads.artifact.path);
                if let Some(rule) = &library.rules {
                    for x in rule {
                        if let Some(os) = &x.os {
                            if let Some(name) = &os.name {
                                if &current_os_type == name {
                                    do_library_clone.store(true, Ordering::SeqCst);
                                }
                            }
                        }
                    }
                }
                if !do_library_clone.load(Ordering::SeqCst) {
                    println!("{} is not required on", library.name);
                    Ok(())
                } else if final_path.exists() {
                    push_clone.store(false, Ordering::Release);
                    println!("{} already downloaded!", library.name);
                    Ok(())
                } else {
                    download_library(library, size_clone, folder).await
                }
            } else {
                Ok(())
            }
        });
        // "race condition"
        // time::sleep(Duration::from_millis(5)).await;
        let push_clone = Arc::clone(&push);
        if push_clone.load(Ordering::Acquire) || do_library_os.load(Ordering::Acquire) {
            library_download_handles.push(handle);
        }
        push_clone.store(true, Ordering::SeqCst);
        do_library_os.store(false, Ordering::SeqCst);
    }

    (library_download_handles, download_total_size)
}
async fn download_file(
    url: String,
    name: Option<PathBuf>,
    current_bytes: Arc<AtomicUsize>,
) -> Result<()> {
    let mut response = reqwest::get(&url).await?;
    let filename = name.map_or_else(
        || extract_filename(&url).unwrap(),
        |x| x.to_str().unwrap().to_string(),
    );
    let mut file = File::create(&filename).await?;
    while let Some(chunk) = response.chunk().await? {
        file.write_all(&chunk).await?;
        current_bytes.fetch_add(chunk.len(), Ordering::Relaxed);
    }
    Ok(())
}

async fn download_library(
    library: &Library,
    current_bytes: Arc<AtomicUsize>,
    folder: Arc<PathBuf>,
) -> Result<()> {
    let path = library.downloads.artifact.path.to_string();
    let path = folder.join(path);
    let parent_dir = std::path::Path::new(&path).parent().unwrap();
    fs::create_dir_all(parent_dir).await?;
    let url = library.downloads.artifact.url.to_string();
    download_file(url, Some(path), current_bytes).await?;
    Ok(())
}

fn extract_filename(url: &str) -> Result<String, Box<dyn std::error::Error>> {
    let parsed_url = Url::parse(url)?;
    let path_segments = parsed_url.path_segments().ok_or("Invalid URL")?;
    let filename = path_segments.last().ok_or("No filename found in URL")?;
    Ok(filename.to_string())
}
