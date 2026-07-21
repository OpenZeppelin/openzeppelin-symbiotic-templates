use std::fs;
use std::path::Path;

use eyre::{Result, eyre};
use serde_json::{Map, Value, json};

use crate::context::ResolvedContext;

/// Fields the split CCV deploy records per chain; source and dest topologies
/// must stay in lockstep.
const CCV_TOPOLOGY_KEYS: &[&str] = &[
    "factory",
    "resolver",
    "verifier",
    "router",
    "rmn",
    "settlement",
    "onRamp",
    "offRamp",
];

pub fn publish(context: &ResolvedContext) -> Result<usize> {
    let deploy_data = context.project_root.join("contracts").join("deploy-data");

    let mut deployments = load_or_default(&context.deployments)?;
    let mut published = 0usize;

    if let Some(value) = read_string(&deploy_data.join("source_contracts.json"), "dvn")? {
        set_path(&mut deployments, &["source", "dvn"], Value::String(value));
        published += 1;
    }
    if let Some(value) = read_string(&deploy_data.join("dest_contracts.json"), "dvn")? {
        set_path(
            &mut deployments,
            &["destination", "dvn"],
            Value::String(value),
        );
        published += 1;
    }
    if let Some(value) = read_object(
        &deploy_data.join("relay_infra.json"),
        &[
            "settlement",
            "driver",
            "keyRegistry",
            "votingPowers",
            "network",
            "stakingToken",
        ],
    )? {
        set_path(&mut deployments, &["destination", "relayInfra"], value);
        published += 1;
    }
    remove_path(&mut deployments, &["source", "testOApp"]);
    remove_path(&mut deployments, &["destination", "testOApp"]);
    remove_path(&mut deployments, &["layerzero", "oapp", "source"]);
    remove_path(&mut deployments, &["layerzero", "oapp", "destination"]);
    if let Some(value) = read_string(&deploy_data.join("example_oapp_source.json"), "oapp")? {
        set_path(
            &mut deployments,
            &["layerzero", "oapp", "source"],
            Value::String(value),
        );
        published += 1;
    }
    if let Some(value) = read_string(&deploy_data.join("example_oapp_dest.json"), "oapp")? {
        set_path(
            &mut deployments,
            &["layerzero", "oapp", "destination"],
            Value::String(value),
        );
        published += 1;
    }
    if let Some(value) = read_object(
        &deploy_data.join("ccv_source_contracts.json"),
        CCV_TOPOLOGY_KEYS,
    )? {
        set_path(&mut deployments, &["source", "chainlinkCcv"], value);
        published += 1;
    }
    if let Some(value) = read_object(
        &deploy_data.join("ccv_dest_contracts.json"),
        CCV_TOPOLOGY_KEYS,
    )? {
        set_path(&mut deployments, &["destination", "chainlinkCcv"], value);
        published += 1;
    }
    if let Some(value) = read_string(&deploy_data.join("example_app_source.json"), "app")? {
        set_path(
            &mut deployments,
            &["source", "chainlinkCcv", "exampleApp"],
            Value::String(value),
        );
        published += 1;
    }
    if let Some(value) = read_string(&deploy_data.join("example_app_dest.json"), "app")? {
        set_path(
            &mut deployments,
            &["destination", "chainlinkCcv", "exampleApp"],
            Value::String(value),
        );
        published += 1;
    }
    if let Some(value) = read_string(&deploy_data.join("noop_executor.json"), "executor")? {
        set_path(
            &mut deployments,
            &["source", "chainlinkCcv", "noOpExecutor"],
            Value::String(value),
        );
        published += 1;
    }

    ensure_parent_dir(&context.deployments)?;
    fs::write(
        &context.deployments,
        format!("{}\n", serde_json::to_string_pretty(&deployments)?),
    )?;

    Ok(published)
}

fn load_or_default(path: &Path) -> Result<Value> {
    if !path.exists() {
        return Ok(json!({
            "source": {},
            "destination": {}
        }));
    }

    let body = fs::read_to_string(path)
        .map_err(|err| eyre!("failed to read deployments {}: {err}", path.display()))?;
    serde_json::from_str(&body)
        .map_err(|err| eyre!("failed to parse deployments {}: {err}", path.display()))
}

fn read_json(path: &Path) -> Result<Option<Value>> {
    if !path.exists() {
        return Ok(None);
    }
    let body = fs::read_to_string(path)
        .map_err(|err| eyre!("failed to read {}: {err}", path.display()))?;
    serde_json::from_str(&body)
        .map(Some)
        .map_err(|err| eyre!("failed to parse {}: {err}", path.display()))
}

fn read_string(path: &Path, key: &str) -> Result<Option<String>> {
    let Some(value) = read_json(path)? else {
        return Ok(None);
    };
    Ok(value
        .get(key)
        .and_then(Value::as_str)
        .map(ToOwned::to_owned))
}

fn read_object(path: &Path, keys: &[&str]) -> Result<Option<Value>> {
    let Some(value) = read_json(path)? else {
        return Ok(None);
    };

    let mut object = Map::new();
    for key in keys {
        if let Some(item) = value.get(*key)
            && !item.is_null()
        {
            object.insert((*key).to_string(), item.clone());
        }
    }
    Ok(Some(Value::Object(object)))
}

fn set_path(root: &mut Value, path: &[&str], value: Value) {
    let mut current = root;
    for segment in &path[..path.len() - 1] {
        if !current.is_object() {
            *current = Value::Object(Map::new());
        }
        current = current
            .as_object_mut()
            .expect("object enforced")
            .entry((*segment).to_string())
            .or_insert_with(|| Value::Object(Map::new()));
    }
    current
        .as_object_mut()
        .expect("object enforced")
        .insert(path[path.len() - 1].to_string(), value);
}

fn remove_path(root: &mut Value, path: &[&str]) {
    let mut current = root;
    for segment in &path[..path.len() - 1] {
        let Some(next) = current.get_mut(*segment) else {
            return;
        };
        current = next;
    }
    let Some(object) = current.as_object_mut() else {
        return;
    };
    object.remove(path[path.len() - 1]);
}

fn ensure_parent_dir(path: &Path) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| eyre!("path has no parent: {}", path.display()))?;
    fs::create_dir_all(parent)?;
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;

    fn write_context() -> ResolvedContext {
        let temp_dir = tempdir().unwrap();
        let root = temp_dir.path().to_path_buf();
        fs::create_dir_all(root.join("contracts").join("deploy-data")).unwrap();
        fs::write(
            root.join("testnet.json"),
            r#"{
                "version": 1,
                "name": "testnet",
                "activeProvider": "layerzero",
                "chains": {
                    "source": { "name": "src", "chainId": 84532, "eid": 40245, "confirmations": 3, "blockTimeMs": 2000, "predeploys": {} },
                    "destination": { "name": "dst", "chainId": 11155111, "eid": 40161, "confirmations": 3, "blockTimeMs": 12000, "predeploys": {} }
                },
                "funding": {
                    "operatorAmountWei": "10000000000000000",
                    "signerAmountWei": "10000000000000000",
                    "minBalanceThresholdWei": "5000000000000000"
                }
            }"#,
        )
        .unwrap();
        std::mem::forget(temp_dir); // keep temp dir alive for test duration

        ResolvedContext {
            project_root: root.clone(),
            env_name: "testnet".to_string(),
            env_config: root.join("testnet.json"),
            deployments: root.join("deployments").join("testnet.json"),
            generated_dir: root.join("generated").join("testnet"),
        }
    }

    #[test]
    fn publish_maps_deploy_data_to_deployments() {
        let context = write_context();
        let deploy_data = context.project_root.join("contracts").join("deploy-data");

        fs::write(
            deploy_data.join("source_contracts.json"),
            r#"{ "dvn": "0x1111111111111111111111111111111111111111" }"#,
        )
        .unwrap();
        fs::write(
            deploy_data.join("dest_contracts.json"),
            r#"{ "dvn": "0x2222222222222222222222222222222222222222" }"#,
        )
        .unwrap();
        fs::write(
            deploy_data.join("relay_infra.json"),
            r#"{
                "settlement": "0x3333333333333333333333333333333333333333",
                "driver": "0xAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
                "keyRegistry": "0x4444444444444444444444444444444444444444",
                "votingPowers": "0x5555555555555555555555555555555555555555",
                "network": "0x6666666666666666666666666666666666666666",
                "stakingToken": "0x7777777777777777777777777777777777777777"
            }"#,
        )
        .unwrap();
        fs::write(
            deploy_data.join("example_oapp_source.json"),
            r#"{ "oapp": "0x8888888888888888888888888888888888888888" }"#,
        )
        .unwrap();
        fs::write(
            deploy_data.join("example_oapp_dest.json"),
            r#"{ "oapp": "0x9999999999999999999999999999999999999999" }"#,
        )
        .unwrap();
        fs::write(
            deploy_data.join("example_app_source.json"),
            r#"{ "app": "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" }"#,
        )
        .unwrap();
        fs::write(
            deploy_data.join("example_app_dest.json"),
            r#"{ "app": "0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb" }"#,
        )
        .unwrap();
        fs::write(
            deploy_data.join("noop_executor.json"),
            r#"{ "executor": "0xcccccccccccccccccccccccccccccccccccccccc" }"#,
        )
        .unwrap();
        fs::write(
            deploy_data.join("ccv_source_contracts.json"),
            r#"{
                "factory": "0x1010101010101010101010101010101010101010",
                "resolver": "0x1111111111111111111111111111111111111110",
                "verifier": "0x1212121212121212121212121212121212121212",
                "router": "0x1313131313131313131313131313131313131313",
                "rmn": "0x1414141414141414141414141414141414141414",
                "settlement": "0x1515151515151515151515151515151515151515",
                "onRamp": "0x1616161616161616161616161616161616161616",
                "offRamp": "0x1717171717171717171717171717171717171717"
            }"#,
        )
        .unwrap();
        fs::write(
            deploy_data.join("ccv_dest_contracts.json"),
            r#"{
                "factory": "0x1010101010101010101010101010101010101010",
                "resolver": "0x1111111111111111111111111111111111111110",
                "verifier": "0x1818181818181818181818181818181818181818",
                "router": "0x1919191919191919191919191919191919191919",
                "rmn": "0x2020202020202020202020202020202020202020",
                "settlement": "0x2121212121212121212121212121212121212121",
                "onRamp": "0x2222222222222222222222222222222222222220",
                "offRamp": "0x2323232323232323232323232323232323232323"
            }"#,
        )
        .unwrap();

        let published = publish(&context).unwrap();
        assert_eq!(published, 10);

        let deployments: Value =
            serde_json::from_str(&fs::read_to_string(&context.deployments).unwrap()).unwrap();
        assert_eq!(
            deployments["source"]["dvn"].as_str(),
            Some("0x1111111111111111111111111111111111111111")
        );
        assert_eq!(
            deployments["destination"]["relayInfra"]["driver"].as_str(),
            Some("0xAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA")
        );
        assert_eq!(
            deployments["layerzero"]["oapp"]["source"].as_str(),
            Some("0x8888888888888888888888888888888888888888")
        );
        assert_eq!(
            deployments["source"]["chainlinkCcv"]["exampleApp"].as_str(),
            Some("0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
        );
        assert_eq!(
            deployments["destination"]["chainlinkCcv"]["exampleApp"].as_str(),
            Some("0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb")
        );
        assert_eq!(
            deployments["source"]["chainlinkCcv"]["noOpExecutor"].as_str(),
            Some("0xcccccccccccccccccccccccccccccccccccccccc")
        );
        assert_eq!(
            deployments["source"]["chainlinkCcv"]["resolver"].as_str(),
            Some("0x1111111111111111111111111111111111111110")
        );
        assert!(deployments["source"]["chainlinkCcv"].get("ccv").is_none());
        assert_eq!(
            deployments["source"]["chainlinkCcv"]["factory"].as_str(),
            Some("0x1010101010101010101010101010101010101010")
        );
        assert_eq!(
            deployments["source"]["chainlinkCcv"]["verifier"].as_str(),
            Some("0x1212121212121212121212121212121212121212")
        );
        assert_eq!(
            deployments["source"]["chainlinkCcv"]["router"].as_str(),
            Some("0x1313131313131313131313131313131313131313")
        );
        assert_eq!(
            deployments["source"]["chainlinkCcv"]["rmn"].as_str(),
            Some("0x1414141414141414141414141414141414141414")
        );
        assert!(!context.generated_dir.join("sidecar.env").exists());
    }
}
