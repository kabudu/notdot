//! Restrict inherited tool approvals without changing the user's global config.
use serde_json::{Value, json};

fn tool_policy(existing: &Value, apps: bool) -> Value {
    // Keep a user's stricter prompt-for-every-call choice.
    let mode = if existing["default_tools_approval_mode"] == "prompt" {
        "prompt"
    } else {
        "writes"
    };
    let mut policy = json!({"default_tools_approval_mode":mode});
    if apps {
        policy["approvals_reviewer"] = "user".into();
    }
    if let Some(tools) = existing["tools"].as_object() {
        for (name, tool) in tools {
            policy["tools"][name] = json!({"approval_mode":if tool["approval_mode"] == "prompt" { "prompt" } else { "writes" }});
        }
    }
    policy
}
pub(crate) fn companion(effective: &Value, effort: &str) -> Value {
    let mut overrides = json!({"model_reasoning_effort":effort,"approvals_reviewer":"user","apps":{"_default":tool_policy(&effective["apps"]["_default"], true)}});
    if let Some(apps) = effective["apps"].as_object() {
        for (id, app) in apps {
            overrides["apps"][id] = tool_policy(app, true);
        }
    }
    if let Some(servers) = effective["mcp_servers"].as_object() {
        for (id, server) in servers {
            overrides["mcp_servers"][id] = tool_policy(server, false);
        }
    }
    if let Some(plugins) = effective["plugins"].as_object() {
        for (id, plugin) in plugins {
            if let Some(servers) = plugin["mcp_servers"].as_object() {
                for (server, config) in servers {
                    overrides["plugins"][id]["mcp_servers"][server] = tool_policy(config, false);
                }
            }
        }
    }
    overrides
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn inherited_auto_approvals_cannot_bypass_the_companion_host_gate() {
        let config = json!({"apps":{"_default":{"default_tools_approval_mode":"approve"},"mail":{"approvals_reviewer":"auto_review","tools":{"send":{"approval_mode":"auto"},"read":{"approval_mode":"prompt"}}}},"mcp_servers":{"local":{"env":{"TOKEN":"synthetic-secret"},"tools":{"write":{"approval_mode":"approve"}}}},"plugins":{"installed":{"mcp_servers":{"remote":{"default_tools_approval_mode":"auto"}}}}});
        let safe = companion(&config, "medium");
        assert_eq!(safe["approvals_reviewer"], "user");
        assert_eq!(
            safe["apps"]["_default"]["default_tools_approval_mode"],
            "writes"
        );
        assert_eq!(safe["apps"]["mail"]["approvals_reviewer"], "user");
        assert_eq!(
            safe["apps"]["mail"]["tools"]["send"]["approval_mode"],
            "writes"
        );
        assert_eq!(
            safe["apps"]["mail"]["tools"]["read"]["approval_mode"],
            "prompt"
        );
        assert_eq!(
            safe["mcp_servers"]["local"]["tools"]["write"]["approval_mode"],
            "writes"
        );
        assert_eq!(
            safe["plugins"]["installed"]["mcp_servers"]["remote"]["default_tools_approval_mode"],
            "writes"
        );
        assert!(!safe.to_string().contains("synthetic-secret"));
        assert_eq!(
            config["apps"]["mail"]["tools"]["send"]["approval_mode"],
            "auto"
        );
    }
}
