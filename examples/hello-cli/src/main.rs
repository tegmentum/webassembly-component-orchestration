use std::env;
use std::process;

fn main() {
    let args: Vec<String> = env::args().collect();

    // Get the name to greet (default to "World")
    let name = if args.len() > 1 {
        &args[1]
    } else {
        "World"
    };

    // Check for special commands
    if name == "--help" || name == "-h" {
        print_help();
        process::exit(0);
    }

    if name == "--version" || name == "-v" {
        println!("hello-cli v0.1.0");
        process::exit(0);
    }

    // Main greeting logic
    eprintln!("[INFO] Starting hello-cli");
    eprintln!("[DEBUG] Arguments received: {}", args.len() - 1);

    println!("Hello, {}!", name);
    println!("Welcome to the WebAssembly Compositional System!");

    eprintln!("[INFO] Greeting complete");
    eprintln!("[METRIC] execution_time_ms: 1");
    eprintln!("[METRIC] output_bytes: {}", name.len() + 50);

    process::exit(0);
}

fn print_help() {
    println!("hello-cli - Simple greeting application");
    println!();
    println!("USAGE:");
    println!("    hello-cli [NAME]");
    println!();
    println!("ARGS:");
    println!("    <NAME>    Name to greet (default: World)");
    println!();
    println!("OPTIONS:");
    println!("    -h, --help       Print help information");
    println!("    -v, --version    Print version information");
}
