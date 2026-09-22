//! Offline developer fixture compiler; these file paths are never native input.
use std::{env, fs};
fn main() {
    let args: Vec<_> = env::args().collect();
    if args.len() != 4 {
        eprintln!("Usage: compile_subscriptions EASYLIST EASYPRIVACY OUTPUT_JSON");
        std::process::exit(2);
    }
    let easylist = fs::read_to_string(&args[1]).expect("read EasyList");
    let easyprivacy = fs::read_to_string(&args[2]).expect("read EasyPrivacy");
    let result = naab_companion::subscription_rules::compile_subscriptions(
        &[("easylist", &easylist), ("easyprivacy", &easyprivacy)],
        27800,
    )
    .expect("compile subscriptions");
    fs::write(&args[3], serde_json::to_vec(&result).unwrap()).expect("write compiled fixture");
    println!(
        "{}",
        serde_json::json!({"stats":result["stats"],"coverage":result["coverage"],"listStats":result["listStats"],"unsupportedReasons":result["unsupportedReasons"]})
    );
}
