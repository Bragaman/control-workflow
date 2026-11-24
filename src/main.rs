use control_workflow::{args, config::Context, core};
use serde_json::json;
fn main() {
    #[cfg(target_os = "linux")]
    if control_workflow::run_temp::inspect_entry() {
        return;
    }
    let matches = args::cli().get_matches();
    let result = (|| {
        if matches.subcommand_name() == Some("_launch") {
            control_workflow::external::seal_launcher_fd()?;
        }
        let context = Context::load(&matches)?;
        let (name, values) = args::values(&matches)?;
        if name == "temp-exec" {
            #[cfg(target_os = "linux")]
            std::process::exit(control_workflow::run_temp::execute(
                &context,
                args::s(&values, "run"),
                values["slot"].as_str().unwrap_or("worker"),
                &args::strings(&values["command"]),
            )?);
            #[cfg(not(target_os = "linux"))]
            anyhow::bail!("temp-exec доступен на Linux");
        }
        if name == "mcp" {
            return control_workflow::mcp::serve(&context);
        }
        let value = core::execute(&context, &name, &values)?;
        println!("{}", serde_json::to_string(&value)?);
        if (["open", "complete", "git-publish"].contains(&name.as_str())
            && control_workflow::lifecycle::failure(&value))
            || (name == "temp-sweep" && value["status"] == "failed")
            || (name == "_complete-request" && value["status"] == "failed")
        {
            std::process::exit(1);
        }
        Ok(())
    })();
    if let Err(error) = result {
        let value = json!({"error":error.to_string(),"error_code":control_workflow::external::error_code(&error)});
        if matches.subcommand_name() == Some("mcp") {
            eprintln!("{value}");
        } else {
            println!("{value}");
        }
        std::process::exit(1);
    }
}
