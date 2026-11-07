//! Evidence-based capability discovery. Missing advertisement is unknown, not unsupported.
//! Inspecting the installed CLI registry never exercises a mutating command.
use crate::{config::Config, process};
use serde_json::{Value, json};
use std::{process::Command, time::Duration};

#[derive(Clone, Copy, Debug, PartialEq)]
enum State {
    Supported,
    Unsupported,
    Unknown,
}

impl State {
    fn name(self) -> &'static str {
        match self {
            Self::Supported => "supported",
            Self::Unsupported => "unsupported",
            Self::Unknown => "unknown",
        }
    }
}

struct Feature {
    name: &'static str,
    runtime_markers: &'static [&'static str],
    command: Option<&'static str>,
    cli_required: bool,
    flags: &'static [&'static str],
    detail: &'static str,
}

const FEATURES: &[Feature] = &[
    Feature {
        name: "terminalTabTransfer",
        runtime_markers: &[crate::session_organization::TAB_TRANSFER_CAPABILITY],
        command: None,
        cli_required: false,
        flags: &[],
        detail: "Native same-host whole-tab transfer; no transfer probe is performed",
    },
    Feature {
        name: "sessionNamingPlans",
        runtime_markers: &[crate::session_organization::SESSION_NAMES_CAPABILITY],
        command: None,
        cli_required: false,
        flags: &[],
        detail: "Native selected naming preview/apply/undo; no naming probe is performed",
    },
    Feature {
        name: "memoryDiagnostics",
        runtime_markers: &["diagnostics.memory"],
        command: Some("diagnostics memory"),
        cli_required: false,
        flags: &[],
        detail: "Direct diagnostics.memory RPC; no expensive process sweep is performed by doctor",
    },
    Feature {
        name: "exactTerminalStop",
        runtime_markers: &["terminal.stopExact"],
        command: None,
        cli_required: false,
        flags: &[],
        detail: "Direct terminal.stopExact RPC; no stop probe is performed",
    },
    Feature {
        name: "launchRetry",
        runtime_markers: &["agent.launch.replay.v1"],
        command: Some("agent launch"),
        cli_required: true,
        flags: &["retry-launch"],
        detail: "Native launch replay requires both host advertisement and CLI retry-launch support",
    },
    Feature {
        name: "promptSubmissionObservation",
        runtime_markers: &["terminal.prompt-delivery.v1"],
        command: Some("terminal send"),
        cli_required: true,
        flags: &["wait-submit"],
        detail: "Observe an accepted prompt without resending; no prompt probe is performed",
    },
    Feature {
        name: "promptRetry",
        runtime_markers: &["terminal.prompt-delivery.v1"],
        command: Some("terminal send"),
        cli_required: true,
        flags: &["retry-request"],
        detail: "Native prompt retry is bound to the original request and terminal incarnation",
    },
    Feature {
        name: "worktreeRemoval",
        runtime_markers: &[],
        command: Some("worktree rm"),
        cli_required: true,
        flags: &["worktree"],
        detail: "CLI availability does not prove host removal support or that a target is safe to remove",
    },
    Feature {
        name: "worktreeRemovalPreview",
        runtime_markers: &[],
        command: None,
        cli_required: false,
        flags: &[],
        detail: "Bridge implements preview locally; required runtime inventories and target safety must still be checked",
    },
    Feature {
        name: "archiveHookFailureBlocking",
        runtime_markers: &["worktree.archive-failure-blocking.v1"],
        command: Some("worktree rm"),
        cli_required: true,
        flags: &["run-hooks", "allow-failed-archive-hook"],
        detail: "Native archive-hook failure blocking and its deliberate override",
    },
];

fn clean_version(value: &Value) -> Option<&str> {
    value.as_str().filter(|value| {
        !value.is_empty() && value.len() <= 128 && !value.chars().any(char::is_control)
    })
}

fn runtime_evidence(status: Option<&Value>, feature: &Feature) -> (State, Vec<Value>) {
    let capabilities = status.and_then(|status| status.get("capabilities"));
    let mut observations = Vec::new();
    for marker in feature.runtime_markers {
        let supported = match capabilities {
            Some(Value::Array(values)) => values
                .iter()
                .any(|value| value.as_str() == Some(marker))
                .then_some(true),
            Some(Value::Object(values)) => values.get(*marker).and_then(|value| {
                value
                    .as_bool()
                    .or_else(|| value.get("supported").and_then(Value::as_bool))
            }),
            _ => None,
        };
        if let Some(supported) = supported {
            observations.push(json!({
                "source":"status.get.capabilities", "name":marker, "supported":supported,
            }));
        }
    }
    let state = if observations.iter().any(|row| row["supported"] == false) {
        State::Unsupported
    } else if observations.iter().any(|row| row["supported"] == true) {
        State::Supported
    } else {
        State::Unknown
    };
    (state, observations)
}

fn registry_valid(registry: &Value) -> bool {
    registry["schemaVersion"] == 1 && registry["commands"].is_array()
}

fn cli_evidence(registry: Option<&Value>, feature: &Feature) -> (Option<State>, Value) {
    let Some(command) = feature.command else {
        return (None, Value::Null);
    };
    let Some(registry) = registry.filter(|registry| registry_valid(registry)) else {
        return (
            Some(State::Unknown),
            json!({"source":"installed-cli.agent-context","command":command,"requiredFlags":feature.flags,"registryAvailable":false}),
        );
    };
    let commands = registry["commands"].as_array().expect("validated registry");
    let matching = commands
        .iter()
        .filter(|row| row["command"] == command)
        .collect::<Vec<_>>();
    let complete = registry["commandCount"].as_u64() == Some(commands.len() as u64);
    let state = match matching.as_slice() {
        [] if complete => State::Unsupported,
        [row] => match row["flags"].as_array() {
            Some(flags)
                if feature
                    .flags
                    .iter()
                    .all(|flag| flags.iter().any(|value| value.as_str() == Some(flag))) =>
            {
                State::Supported
            }
            Some(_) => State::Unsupported,
            None => State::Unknown,
        },
        _ => State::Unknown,
    };
    (
        Some(state),
        json!({"source":"installed-cli.agent-context","command":command,"requiredFlags":feature.flags,"registryAvailable":true,"registryComplete":complete}),
    )
}

fn combine(runtime: State, cli: Option<State>, target_mismatch: bool) -> State {
    if runtime == State::Unsupported || cli == Some(State::Unsupported) {
        State::Unsupported
    } else if target_mismatch && cli.is_some() {
        State::Unknown
    } else if runtime == State::Supported && cli.is_none_or(|state| state == State::Supported) {
        State::Supported
    } else {
        State::Unknown
    }
}

fn report(status: Option<&Value>, registry: Option<&Value>, target_mismatch: bool) -> Value {
    let valid_registry = registry.filter(|registry| registry_valid(registry));
    let features = FEATURES
        .iter()
        .map(|feature| {
            let (runtime, runtime_evidence) = runtime_evidence(status, feature);
            let (cli, cli_evidence) = cli_evidence(valid_registry, feature);
            let state = combine(
                runtime,
                cli.filter(|_| feature.cli_required),
                target_mismatch && feature.cli_required,
            );
            (
                feature.name.to_owned(),
                json!({
                        "state":state.name(), "runtimeState":runtime.name(),
                        "cliState":cli.map(State::name), "runtimeEvidence":runtime_evidence,
                        "cliEvidence":cli_evidence, "detail":feature.detail,
                "cliRequired":feature.cli_required,
                        "bridgeImplemented":true,
                    }),
            )
        })
        .collect::<serde_json::Map<_, _>>();
    let mut warnings = vec!["Missing runtime advertisement remains unknown; feature availability is not a safe-to-act verdict".to_owned()];
    if valid_registry.is_none() {
        warnings.push("Installed CLI schema was unavailable or incompatible; CLI-dependent capabilities remain unknown".into());
    }
    if target_mismatch {
        warnings.push("Native CLI remote targeting is configured but status describes the local RPC runtime; combined CLI/host support is unknown".into());
    }
    json!({
        "schemaVersion":1, "features":features,
        "runtimeAvailable":status.is_some(), "cliRegistryAvailable":valid_registry.is_some(),
        "runtimeAppVersion":status.and_then(|status| clean_version(&status["appVersion"])),
        "cliVersion":valid_registry.and_then(|registry| clean_version(&registry["cliVersion"]).or_else(|| clean_version(&registry["version"]))),
        "cliTargetMatchesLocalRuntime":if target_mismatch {Value::Bool(false)} else {Value::Null},
        "warnings":warnings,
    })
}

/// Inspect advertised host support and the installed CLI's offline command registry.
/// No diagnostic sweep, prompt, launch, stop, archive hook or deletion is executed.
pub fn inspect(config: &Config, status: Option<&Value>) -> Value {
    let mut command = Command::new(&config.orca_cli);
    command
        .args(["agent-context", "--json"])
        .env("ORCA_USER_DATA_PATH", &config.data_dir);
    let registry = process::command_limits()
        .ok()
        .and_then(|(timeout, cap)| {
            process::output(
                &mut command,
                timeout.min(Duration::from_secs(2)),
                cap.min(2 * 1024 * 1024),
            )
            .ok()
        })
        .filter(|output| output.status.success())
        .and_then(|output| serde_json::from_slice::<Value>(&output.stdout).ok())
        .map(|value| value.get("result").cloned().unwrap_or(value));
    let target_mismatch = ["ORCA_PAIRING_CODE", "ORCA_ENVIRONMENT"]
        .iter()
        .any(|name| std::env::var_os(name).is_some_and(|value| !value.is_empty()));
    report(status, registry.as_ref(), target_mismatch)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry() -> Value {
        json!({"schemaVersion":1,"commandCount":3,"commands":[
            {"command":"agent launch","flags":["retry-launch"]},
            {"command":"terminal send","flags":["wait-submit","retry-request"]},
            {"command":"worktree rm","flags":["worktree","run-hooks","allow-failed-archive-hook"]},
        ]})
    }

    #[test]
    fn current_advertisement_is_combined_with_cli_flags_without_inventing_support() {
        let status = json!({"appVersion":"fixture","capabilities":["agent.launch.replay.v1","terminal.prompt-delivery.v1","worktree.archive-failure-blocking.v1"]});
        let report = report(Some(&status), Some(&registry()), false);
        for name in [
            "launchRetry",
            "promptSubmissionObservation",
            "promptRetry",
            "archiveHookFailureBlocking",
        ] {
            assert_eq!(report["features"][name]["state"], "supported");
        }
        for name in [
            "memoryDiagnostics",
            "exactTerminalStop",
            "worktreeRemoval",
            "worktreeRemovalPreview",
        ] {
            assert_eq!(report["features"][name]["state"], "unknown");
        }
        assert_eq!(
            report["features"]["worktreeRemoval"]["cliState"],
            "supported"
        );
    }

    #[test]
    fn older_missing_and_unrelated_capabilities_stay_unknown_despite_installed_cli() {
        for status in [
            json!({"appVersion":"9999"}),
            json!({"capabilities":["unrelated.feature.v1"]}),
            json!({"capabilities":[]}),
        ] {
            let report = report(Some(&status), Some(&registry()), false);
            assert_eq!(report["features"]["launchRetry"]["state"], "unknown");
            assert_eq!(report["features"]["promptRetry"]["state"], "unknown");
        }
        let report = report(None, Some(&registry()), false);
        assert_eq!(report["runtimeAvailable"], false);
        assert_eq!(report["features"]["launchRetry"]["state"], "unknown");
    }

    #[test]
    fn explicit_runtime_false_is_unsupported_without_a_mutating_probe() {
        let status = json!({"capabilities":{"terminal.stopExact":false,"agent.launch.replay.v1":{"supported":false},"diagnostics.memory":true}});
        let report = report(Some(&status), Some(&registry()), false);
        assert_eq!(
            report["features"]["exactTerminalStop"]["state"],
            "unsupported"
        );
        assert_eq!(report["features"]["launchRetry"]["state"], "unsupported");
        assert_eq!(
            report["features"]["memoryDiagnostics"]["state"],
            "supported"
        );
    }

    #[test]
    fn cli_missing_flags_and_complete_missing_commands_are_explicitly_unsupported() {
        let status =
            json!({"capabilities":["terminal.prompt-delivery.v1","agent.launch.replay.v1"]});
        let registry = json!({"schemaVersion":1,"commandCount":1,"commands":[{"command":"terminal send","flags":["text"]}]});
        let report = report(Some(&status), Some(&registry), false);
        assert_eq!(report["features"]["promptRetry"]["state"], "unsupported");
        assert_eq!(report["features"]["launchRetry"]["cliState"], "unsupported");
    }

    #[test]
    fn cli_schema_failure_or_target_mismatch_never_proves_host_support() {
        let status = json!({"capabilities":["terminal.prompt-delivery.v1","agent.launch.replay.v1"],"authToken":"private-fixture"});
        for registry in [
            None,
            Some(json!({"schemaVersion":99,"commands":[]})),
            Some(json!({"schemaVersion":1,"commands":[]})),
        ] {
            let report = report(Some(&status), registry.as_ref(), false);
            assert_eq!(report["features"]["launchRetry"]["state"], "unknown");
            assert!(!report.to_string().contains("private-fixture"));
        }
        let report = report(Some(&status), Some(&registry()), true);
        assert_eq!(report["features"]["launchRetry"]["state"], "unknown");
        assert_eq!(
            report["features"]["launchRetry"]["runtimeState"],
            "supported"
        );
        assert_eq!(report["cliTargetMatchesLocalRuntime"], false);
    }

    #[test]
    fn inspect_executes_only_the_offline_schema_command_and_keeps_evidence_separate() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let cli = dir.path().join("fixture-orca");
        let script = format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$ORCA_USER_DATA_PATH/invoked\"\nprintf '%s\\n' '{}'\n",
            registry()
        );
        std::fs::write(&cli, script).unwrap();
        std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o755)).unwrap();
        let config = Config {
            data_dir: dir.path().into(),
            state_dir: dir.path().join("state"),
            orca_cli: cli,
            profile_db_override: None,
        };
        let status = json!({"capabilities":["agent.launch.replay.v1"]});
        let result = inspect(&config, Some(&status));
        assert_eq!(
            std::fs::read_to_string(dir.path().join("invoked")).unwrap(),
            "agent-context\n--json\n"
        );
        assert_eq!(result["cliRegistryAvailable"], true);
        assert_eq!(
            result["features"]["launchRetry"]["runtimeState"],
            "supported"
        );
        assert_eq!(result["features"]["launchRetry"]["cliState"], "supported");
        assert!(!config.state_dir.exists());
    }
}
