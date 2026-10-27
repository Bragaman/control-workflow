use control_workflow::{args, config::Context, review, store};
use serde_json::{Value, json};
use std::{path::Path, process::Command};
fn context(dir: &Path) -> Context {
    std::fs::write(dir.join("config.json"), "{}").unwrap();
    let m = args::cli()
        .try_get_matches_from([
            "control-workflow",
            "--state-root",
            dir.join("state").to_str().unwrap(),
            "--bindings-root",
            dir.join("bindings").to_str().unwrap(),
            "--config",
            dir.join("config.json").to_str().unwrap(),
            "environment",
        ])
        .unwrap();
    Context::load(&m).unwrap()
}

#[test]
fn portable_policy_validation_precedence_and_legacy_compatibility() {
    use control_workflow::{core, policy};
    let temp = tempfile::tempdir().unwrap();
    let mut c = context(temp.path());
    let task = json!({"task_kind":"research","complexity":"complex"});
    let legacy = core::profile(&c, &task).unwrap();
    assert_eq!(legacy["model"], "default");
    assert_eq!(legacy["model_selection"]["source"], "legacy");
    let local = json!({"model_policy":"builtin","model_overrides":{"profiles":{"luna":{"reasoning_effort":"high"}}}});
    policy::load(&mut c.settings, None, &local).unwrap();
    let auto = core::profile(&c, &task).unwrap();
    assert_eq!(auto["profile"], "sol-high");
    let mut selected = task.clone();
    selected["profile"] = json!("terra");
    assert_eq!(
        core::profile(&c, &selected).unwrap()["model"],
        "gpt-6.1-terra"
    );
    let run = core::profile_for_run(&c, &selected, Some("luna")).unwrap();
    assert_eq!(run["model"], "gpt-6-luna");
    assert_eq!(run["reasoning_effort"], "high");
    assert_eq!(run["model_selection"]["source"], "run");
    assert!(policy::check_reuse(Some(&run), &auto).is_err());
    assert!(policy::check_reuse(Some(&run), &run).is_ok());
    assert!(core::profile_for_run(&c, &task, Some("missing")).is_err());
    for bad in [
        json!({"model_policy":null}),
        json!({"model_policy":"builtin","profiles":{}}),
        json!({"model_overrides":{}}),
        json!({"model_policy":"builtin","model_overrides":{"cwd":"/host"}}),
        json!({"model_policy":"builtin","model_overrides":{"profiles":{"luna":{"reasoning_effort":"typo"}}}}),
        json!({"model_policy":"builtin","model_overrides":{"profiles":{"luna":{"model":"default"}}}}),
        json!({"model_policy":"builtin","model_overrides":{"profiles":{"luna":{"model":"/bad model"}}}}),
        json!({"model_policy":"builtin","model_overrides":{"rules":{"research":{"normal":"typo"}}}}),
        json!({"model_policy":"builtin","model_overrides":{"profiles":null}}),
        json!({"model_policy":"builtin","allowed_models":"sol"}),
    ] {
        assert!(
            policy::load(&mut c.settings.clone(), None, &bad).is_err(),
            "accepted {bad}"
        );
    }
    c.settings["allowed_models"] = json!(["gpt-6-luna"]);
    assert!(core::profile(&c, &task).is_err());
    assert!(core::profile_for_run(&c, &task, Some("luna")).is_ok());
}
#[test]
fn preserve_unknown_fields_and_split_bindings() {
    let temp = tempfile::tempdir().unwrap();
    let c = context(temp.path());
    let f = c.path("projects", "demo").unwrap();
    store::save_raw(&f,&json!({"id":"demo","name":"Demo","unknown":{"值":[1,true]},"workspace":"w1","repositories":[]})).unwrap();
    store::save_raw(
        &c.bindings.join("projects/demo.json"),
        &json!({"future_binding":{"key":42}}),
    )
    .unwrap();
    let mut v = c.read(&f).unwrap();
    v["description"] = json!("edited");
    c.save(&f, &v).unwrap();
    let round = c.read(&f).unwrap();
    assert_eq!(round["unknown"], json!({"值":[1,true]}));
    assert_eq!(round["future_binding"], json!({"key":42}));
    assert!(store::raw(&f).unwrap().get("workspace").is_none());
}
#[test]
fn identifier_traversal_and_empty_are_rejected() {
    for key in ["../escape", "", "UPPER", "-leading", "a/b"] {
        assert!(store::ident(key).is_err());
    }
}
#[test]
fn python_canonical_separators_and_unicode() {
    let v = json!({"z": [true, null, 2], "a": "Русский\n\"\\\t"});
    assert_eq!(
        String::from_utf8(review::canonical(&v)).unwrap(),
        "{\"a\": \"Русский\\n\\\"\\\\\\t\", \"z\": [true, null, 2]}"
    );
}
#[test]
fn native_package_is_independent_and_rejects_foreign_files() {
    let temp = tempfile::tempdir().unwrap();
    let c = context(temp.path());
    let exe = env!("CARGO_BIN_EXE_control-workflow");
    let prefix = temp.path().join("standalone");
    let result = Command::new(exe)
        .args([
            "--state-root",
            c.root.to_str().unwrap(),
            "--bindings-root",
            c.bindings.to_str().unwrap(),
            "--config",
            c.config.as_ref().unwrap().to_str().unwrap(),
            "package",
            "--prefix",
            prefix.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stdout)
    );
    let installed = prefix.join("bin/control-workflow");
    let result = Command::new(&installed)
        .arg("--state-root")
        .arg(&c.root)
        .arg("--bindings-root")
        .arg(&c.bindings)
        .arg("--config")
        .arg(c.config.as_ref().unwrap())
        .arg("environment")
        .env("PATH", "/nonexistent")
        .output()
        .unwrap();
    assert!(result.status.success());
    let env: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(env["program_root"], json!(prefix));
    assert!(!prefix.join("scripts").exists());
    for name in [
        "README.md",
        "AGENTS.md",
        "docs/install.md",
        "docs/control-entry.md",
        "examples/config-codex.json",
        "examples/task.md",
        "skills/control-sync/SKILL.md",
    ] {
        assert_eq!(
            std::fs::read(prefix.join(name)).unwrap(),
            std::fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join(name)).unwrap(),
            "shipped resource differs: {name}"
        );
    }
    std::fs::write(prefix.join("personal.txt"), "keep").unwrap();
    let result = Command::new(exe)
        .arg("--state-root")
        .arg(&c.root)
        .arg("--bindings-root")
        .arg(&c.bindings)
        .arg("install")
        .arg("--prefix")
        .arg(&prefix)
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert_eq!(
        std::fs::read_to_string(prefix.join("personal.txt")).unwrap(),
        "keep"
    );
}

#[test]
fn canonical_numbers_match_python_and_large_integers_survive() {
    let v: Value = serde_json::from_str(
        r#"[1e-6, 1e-5, 1e-4, 1e15, 1e16, 1.20000, -0.0, 123456789012345678901234567890]"#,
    )
    .unwrap();
    assert_eq!(
        String::from_utf8(review::canonical(&v)).unwrap(),
        "[1e-06, 1e-05, 0.0001, 1000000000000000.0, 1e+16, 1.2, -0.0, 123456789012345678901234567890]"
    );
}

#[test]
fn unicode_casefold_matches_search_contract() {
    assert_eq!(
        control_workflow::casefold::fold("Straße Σς ﬀ"),
        "strasse σσ ff"
    );
}

#[test]
fn paths_resolve_parent_components_and_broken_symlinks() {
    let temp = tempfile::tempdir().unwrap();
    let input = temp.path().join("missing/../state");
    assert_eq!(
        control_workflow::config::location(&input).unwrap(),
        temp.path().join("state")
    );
    std::os::unix::fs::symlink("absent-target", temp.path().join("link")).unwrap();
    assert_eq!(
        control_workflow::config::location(temp.path().join("link")).unwrap(),
        temp.path().join("absent-target")
    );
}

#[test]
fn empty_herdr_ack_is_scoped_to_commands_without_result() {
    let temp = tempfile::tempdir().unwrap();
    let mut c = context(temp.path());
    let fake = temp.path().join("herdr");
    std::fs::write(&fake, "#!/bin/sh\nexit 0\n").unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    c.machine["herdr_executable"] = json!(fake);
    c.session = Some("isolated".into());
    assert!(c.herdr(&["pane", "rename", "w1:p1", "label"], None).is_ok());
    for args in [
        &["workspace", "create"][..],
        &["agent", "start"][..],
        &["pane", "current"][..],
    ] {
        assert!(c.herdr(args, None).is_err());
    }
}

#[test]
fn installation_update_backup_and_skill_conflicts_are_preserved() {
    let temp = tempfile::tempdir().unwrap();
    let c = context(temp.path());
    let exe = env!("CARGO_BIN_EXE_control-workflow");
    let prefix = temp.path().join("program");
    let invoke = |binary: &Path, args: &[&str]| {
        Command::new(binary)
            .arg("--state-root")
            .arg(&c.root)
            .arg("--bindings-root")
            .arg(&c.bindings)
            .arg("--config")
            .arg(c.config.as_ref().unwrap())
            .args(args)
            .output()
            .unwrap()
    };
    assert!(
        invoke(
            Path::new(exe),
            &["install", "--prefix", prefix.to_str().unwrap()]
        )
        .status
        .success()
    );
    std::fs::create_dir_all(&c.root).unwrap();
    std::fs::write(c.root.join("checkpoint.md"), "saved").unwrap();
    let updated = invoke(
        Path::new(exe),
        &["install", "--prefix", prefix.to_str().unwrap()],
    );
    assert!(updated.status.success());
    let output: Value = serde_json::from_slice(&updated.stdout).unwrap();
    assert!(Path::new(output["backup"].as_str().unwrap()).is_dir());
    assert_eq!(
        std::fs::read_to_string(c.root.join("checkpoint.md")).unwrap(),
        "saved"
    );
    let installed = prefix.join("bin/control-workflow");
    let dest = temp.path().join("skills");
    let args = ["skills", "--destination", dest.to_str().unwrap()];
    assert!(invoke(&installed, &args).status.success());
    assert_eq!(std::fs::read_dir(&dest).unwrap().count(), 8);
    assert!(
        invoke(
            &installed,
            &[
                "skills",
                "--destination",
                dest.to_str().unwrap(),
                "--action",
                "check"
            ]
        )
        .status
        .success()
    );
    let link = dest.join("control-task");
    std::fs::remove_file(&link).unwrap();
    std::os::unix::fs::symlink(temp.path().join("foreign"), &link).unwrap();
    assert!(
        !invoke(
            &installed,
            &[
                "skills",
                "--destination",
                dest.to_str().unwrap(),
                "--action",
                "remove"
            ]
        )
        .status
        .success()
    );
    assert_eq!(std::fs::read_dir(&dest).unwrap().count(), 8);
    let alias = temp.path().join("alias");
    std::os::unix::fs::symlink(&prefix, &alias).unwrap();
    assert!(
        !invoke(
            Path::new(exe),
            &["install", "--prefix", alias.to_str().unwrap()]
        )
        .status
        .success()
    );
}

#[test]
fn canonical_halfway_floats_match_python_rounding() {
    let v: Value =
        serde_json::from_str("[1461205785224690.2, -1250201187231379.2, 11949781134863.562]")
            .unwrap();
    assert_eq!(
        String::from_utf8(review::canonical(&v)).unwrap(),
        "[1461205785224690.2, -1250201187231379.2, 11949781134863.562]"
    );
}

#[test]
fn relocated_colon_paths_keep_targets_and_external_urls() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("reports/report.md");
    let destination = temp.path().join("memory/lab/result.md");
    let text = "[data](attachments/result:2026.txt?download=1#part)\n[web](https://example.test/a:b) [mail](mailto:person@example.test) [anchor](#part)\n`[code](attachments/result:2026.txt)`\n";
    let relocated = control_workflow::core::relocate(text, &source, &destination).unwrap();
    assert_eq!(
        relocated,
        "[data](../../reports/attachments/result%3A2026.txt?download=1#part)\n[web](https://example.test/a:b) [mail](mailto:person@example.test) [anchor](#part)\n`[code](attachments/result:2026.txt)`\n"
    );
}

#[test]
fn canonical_integer_zero_is_distinct_from_float_signed_zero() {
    let value: Value = serde_json::from_str(r#"[-0, 0, -0.0, -0e+0, 0.0, "-0"]"#).unwrap();
    assert_eq!(
        String::from_utf8(review::canonical(&value)).unwrap(),
        r#"[0, 0, -0.0, -0.0, 0.0, "-0"]"#
    );
}

#[test]
fn python_nonfinite_file_json_preserves_canonical_and_strings() {
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("python.json");
    let input = r#"{"values":[Infinity,-Infinity,NaN,1e309,-1e309,-0,-0.0],"text":"NaN Infinity -Infinity \\\"", "0.1234567890123456789":"untouched"}"#;
    std::fs::write(&file, input).unwrap();
    let value = store::raw(&file).unwrap();
    assert_eq!(
        String::from_utf8(review::canonical(&value)).unwrap(),
        r#"{"0.1234567890123456789": "untouched", "text": "NaN Infinity -Infinity \\\"", "values": [Infinity, -Infinity, NaN, Infinity, -Infinity, 0, -0.0]}"#
    );
    store::save_raw(&file, &value).unwrap();
    let roundtrip = store::raw(&file).unwrap();
    assert_eq!(review::canonical(&roundtrip), review::canonical(&value));
    for bad in [
        "-NaN",
        "+Infinity",
        "--Infinity",
        "Infinityx",
        "[NaN,]",
        "{Infinity:0}",
        "NaN 0",
        "[inf]",
        "[nan]",
    ] {
        std::fs::write(&file, bad).unwrap();
        assert!(store::raw(&file).is_err(), "accepted {bad}");
    }
}

#[test]
fn read_outputs_keep_nonfinite_unknown_fields() {
    let temp = tempfile::tempdir().unwrap();
    let c = context(temp.path());
    let memory = c.root.join("memory/demo/mem-demo.json");
    store::atomic(&memory, br#"{"id":"mem-demo","project":"demo","title":"Note","file":"memory/demo/mem-demo.md","future":NaN}"#).unwrap();
    std::fs::write(memory.with_extension("md"), "body").unwrap();
    for parameters in [
        json!({"project":"demo"}),
        json!({"project":"demo","limit":10}),
    ] {
        let parameters = args::defaults("memory-list", parameters).unwrap();
        let value = control_workflow::core::execute(&c, "memory-list", &parameters).unwrap();
        let item = if value.is_array() {
            &value[0]
        } else {
            &value["items"][0]
        };
        assert_eq!(item["future"].to_string(), "NaN");
    }
    store::atomic(
        &c.path("notifications", "notice-demo").unwrap(),
        br#"{"title":"Notice","future":Infinity,"acknowledged":false}"#,
    )
    .unwrap();
    let value = control_workflow::core::execute(
        &c,
        "notifications",
        &args::defaults("notifications", json!({})).unwrap(),
    )
    .unwrap();
    assert_eq!(value[0]["future"].to_string(), "Infinity");
}

#[test]
fn native_process_identity_detects_previous_boot_and_pid_reuse() {
    use control_workflow::external;
    let pid = std::process::id() as i64;
    let identity = external::identity(pid).expect("current process identity");
    assert!(external::process_running(pid));
    let mut worker = json!({"worker_pid":pid,"worker_identity":identity});
    assert!(external::launcher_alive(&worker));
    worker["worker_identity"]["boot_id"] = json!("previous-boot");
    assert!(!external::launcher_alive(&worker));
    worker["worker_identity"] = external::identity(pid).unwrap();
    #[cfg(target_os = "linux")]
    {
        worker["worker_identity"]["start_ticks"] = json!("previous-process");
    }
    #[cfg(target_os = "macos")]
    {
        worker["worker_identity"]["start_usec"] =
            json!(worker["worker_identity"]["start_usec"].as_u64().unwrap() + 1);
    }
    assert!(!external::launcher_alive(&worker));
    assert!(!external::request_alive(
        &json!({"pid":pid,"identity":null})
    ));
}
