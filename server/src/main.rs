use dotenvy::dotenv;

#[tokio::main]
async fn main() {
    let _ = dotenv();
    if let Err(e) = kamism_lib::start_server().await {
        eprintln!("服务器启动失败: {}", e);
        std::process::exit(1);
    }
}
