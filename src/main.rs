use control_workflow::{args, config::Context, core};
use serde_json::json;
fn main() {
    let matches = args::cli().get_matches();
    let result = (|| {
        let context = Context::load(&matches)?;
        let (name, values) = args::values(&matches)?;
        if name == "mcp" {
            return control_workflow::mcp::serve(&context);
        }
        let value = core::execute(&context, &name, &values)?;
        println!("{}", serde_json::to_string(&value)?);
        if (["open", "complete"].contains(&name.as_str())
            && control_workflow::lifecycle::failure(&value))
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
