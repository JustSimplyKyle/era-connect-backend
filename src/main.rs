use std::collections::HashMap;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, PartialEq, Serialize, Deserialize)]
enum ActionType {
    Allow,
    Disallow,
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
enum OsName {
    Osx,
    Windows,
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
    features: Option<std::collections::HashMap<String, bool>>,
    os: Option<Os>,
    value: Option<Vec<String>>,
}

// struct AssetIndex {
//     id: String,
//     sha1: String,
//     size: f32,
//     total_size: f32,
//     url: String,
// }

// struct LibraryDownload {
//     path: String,
//     sha1: String,
//     size: f32,
//     url: String,
// }
// struct MojangLibrary {

// }

// struct GameArguments {

// }
// struct JvmArguments {

// }
fn get_rules(argument: &[Value]) -> Vec<Rule> {
    let mut rules: Vec<Rule> = Vec::new();
    for item in argument.iter() {
        let object = item["rules"][0].as_object();
        if let Some(x) = object {
            let features = x.get("features").map(|y| {
                y.as_object()
                    .unwrap()
                    .iter()
                    .map(|x| (x.0.clone(), x.1.as_bool().unwrap()))
                    .collect::<HashMap<_, _>>()
            });
            rules.push(Rule {
                action: match x.get("action").unwrap().as_str().unwrap() {
                    "allow" => ActionType::Allow,
                    "disallow" => ActionType::Disallow,
                    _ => unreachable!(),
                },
                features,
                os: if let Some(y) = x.get("os") {
                    if let Some(obj) = y.as_object().unwrap().iter().next() {
                        Some(Os {
                            name: match obj.0.as_str() {
                                "name" => match obj.1.as_str().unwrap() {
                                    "osx" => Some(OsName::Osx),
                                    "windows" => Some(OsName::Windows),
                                    "arch" => Some(OsName::Linux),
                                    _ => unreachable!(),
                                },
                                _ => None,
                            },
                            arch: match obj.0.as_str() {
                                "arch" => Some(obj.1.to_string()),
                                _ => None,
                            },
                            version: match obj.0.as_str() {
                                "version" => Some(obj.1.to_string()),
                                _ => None,
                            },
                        })
                    } else {
                        unreachable!();
                    }
                } else {
                    None
                },
                value: item["value"]
                    .as_array()
                    .map(|str_vec| str_vec.iter().map(|x| x.to_string()).collect()),
            });
        }
    }
    rules
}

#[tokio::main]
async fn main() -> Result<()> {
    let target = "https://piston-meta.mojang.com/v1/packages/715ccf3330885e75b205124f09f8712542cbe7e0/1.20.1.json";
    let response = reqwest::get(target).await?;
    let contents: Value = response.json().await?;

    let game_argument = contents["arguments"]["game"].as_array().unwrap();
    let game_rules = get_rules(game_argument);

    let jvm_argument = contents["arguments"]["jvm"].as_array().unwrap();
    let jvm_rules = get_rules(jvm_argument);

    dbg!(game_rules, jvm_rules);
    Ok(())
}
