fn main() {
    let version = libagent::version();

    let args: Vec<String> = std::env::args().collect();

    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("runa {version}");
        return;
    }

    println!("runa {version}");
}
