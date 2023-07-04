use std::collections::HashMap;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

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
    size: u64,
    #[serde(rename = "totalSize")]
    total_size: u64,
    url: String,
}

#[derive(Debug, PartialEq, Serialize, Deserialize, Default)]
struct DownloadMetadata {
    sha1: String,
    size: u64,
    url: String,
}

#[derive(Debug, PartialEq, Serialize, Deserialize, Default)]
struct Downloads {
    client: DownloadMetadata,
    client_mappings: DownloadMetadata,
    server: DownloadMetadata,
    server_mappings: DownloadMetadata,
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

    dbg!(downloads_list, asset_index, game_flags, jvm_flags);
    Ok(())
}
