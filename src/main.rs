use std::collections::HashMap;

use anyhow::{anyhow, Result};
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

fn get_rules(argument: &[Value]) -> Result<Vec<Rule>> {
    let mut rules: Vec<Rule> = Vec::new();
    for item in argument.iter() {
        let object = item["rules"][0]
            .as_object()
            .ok_or_else(|| anyhow!("Expected 'rules' to be an array with at least one object."))?;

        let features = object.get("features").map(|y| {
            y.as_object()
                .unwrap()
                .iter()
                .map(|x| (x.0.clone(), x.1.as_bool().unwrap()))
                .collect::<HashMap<_, _>>()
        });

        let os = object.get("os").and_then(|y| {
            y.as_object().unwrap().iter().next().map(|(k, v)| {
                let name = match k.as_str() {
                    "name" => v.as_str().and_then(|name| match name {
                        "osx" => Some(OsName::Osx),
                        "windows" => Some(OsName::Windows),
                        "arch" => Some(OsName::Linux),
                        _ => None,
                    }),
                    _ => None,
                };

                let arch = match k.as_str() {
                    "arch" => v.as_str().map(String::from),
                    _ => None,
                };

                let version = match k.as_str() {
                    "version" => v.as_str().map(String::from),
                    _ => None,
                };

                Os {
                    name,
                    arch,
                    version,
                }
            })
        });
        rules.push(Rule {
            action: match object.get("action").unwrap().as_str().unwrap() {
                "allow" => ActionType::Allow,
                "disallow" => ActionType::Disallow,
                _ => return Err(anyhow!("Invalid 'action' value.")),
            },
            features,
            os,
            value: item["value"]
                .as_array()
                .map(|str_vec| str_vec.iter().map(|x| x.to_string()).collect()),
        });
    }
    Ok(rules)
}

#[tokio::main]
async fn main() -> Result<()> {
    let target = "https://piston-meta.mojang.com/v1/packages/715ccf3330885e75b205124f09f8712542cbe7e0/1.20.1.json";
    let response = reqwest::get(target).await?;
    let contents: Value = response.json().await?;

    let game_argument = contents["arguments"]["game"].as_array().unwrap();
    let game_rules = get_rules(game_argument)?;

    let jvm_argument = contents["arguments"]["jvm"].as_array().unwrap();
    let jvm_rules = get_rules(jvm_argument)?;

    dbg!(game_rules, jvm_rules);
    Ok(())
}
