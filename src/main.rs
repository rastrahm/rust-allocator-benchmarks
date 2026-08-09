use allocator_benchmarks::accelerator::{AcceleratorConfig, AcceleratorMode};
use allocator_benchmarks::ingress::{IngressConfig, IngressMode};
use allocator_benchmarks::runner::{run_benchmark, BenchmarkConfig};
use allocator_benchmarks::workload::PacketFormat;
use clap::Parser;

#[cfg(feature = "use-jemalloc")]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

#[cfg(feature = "use-mimalloc")]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

/// CLI del benchmark suite de allocators de memoria.
#[derive(Parser, Debug)]
#[command(name = "allocator-benchmarks")]
#[command(about = "High-throughput memory allocator benchmark suite")]
struct Cli {
    /// Número de hilos worker consumidores.
    #[arg(short = 't', long = "threads")]
    threads: Option<usize>,

    /// Cantidad total de payloads a procesar.
    #[arg(short = 'n', long = "iterations", default_value_t = 10_000)]
    iterations: u64,

    /// Tamaño de cada payload simulado en bytes.
    #[arg(short = 's', long = "payload-size", default_value_t = 2_048)]
    payload_bytes: usize,

    /// Formato de parsing simulado.
    #[arg(short = 'f', long = "format", value_enum, default_value_t = PacketFormat::JsonLike)]
    format: PacketFormat,

    /// Semilla base para contenido determinista del payload.
    #[arg(long, default_value_t = 0)]
    seed: u64,

    /// Capacidad del canal acotado entre productor y consumidores.
    #[arg(long = "queue-depth")]
    queue_depth: Option<usize>,

    /// Modo de ingesta de red.
    #[arg(long = "ingress", value_enum, default_value_t = IngressMode::InMemory)]
    ingress: IngressMode,

    /// Puerto TCP para modos async-tcp / io-uring.
    #[arg(long = "ingress-port", default_value_t = 9_876)]
    ingress_port: u16,

    /// Conexiones cliente concurrentes (modos de red).
    #[arg(long = "ingress-connections", default_value_t = 4)]
    ingress_connections: usize,

    /// Tamaño de batch de lectura por conexión.
    #[arg(long = "ingress-read-batch", default_value_t = 64)]
    ingress_read_batch: usize,

    /// Modo de aceleración GPU post-procesamiento.
    #[arg(long = "accelerator", value_enum, default_value_t = AcceleratorMode::None)]
    accelerator: AcceleratorMode,

    /// Índice de dispositivo CUDA.
    #[arg(long = "cuda-device", default_value_t = 0)]
    cuda_device: u32,

    /// Tamaño de batch para kernels GPU.
    #[arg(long = "gpu-batch-size", default_value_t = 256)]
    gpu_batch_size: usize,

    /// Muestra el allocator activo en stderr.
    #[arg(long, default_value_t = false)]
    verbose: bool,
}

/// Punto de entrada del binario.
///
/// # Returns
///
/// `Ok(())` si la ejecución termina correctamente, o un error de aplicación.
fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    if cli.verbose {
        eprintln!("allocator: {}", active_allocator_name());
    }

    let config = build_config(&cli)?;
    let result = run_benchmark(&config)?;

    allocator_benchmarks::runner::print_results(active_allocator_name(), &config, &result);
    Ok(())
}

/// Construye la configuración del benchmark a partir de los argumentos CLI.
fn build_config(cli: &Cli) -> anyhow::Result<BenchmarkConfig> {
    let default = BenchmarkConfig::default();
    let threads = cli.threads.unwrap_or(default.threads);
    let queue_depth = cli.queue_depth.unwrap_or(threads * 4);

    Ok(BenchmarkConfig {
        threads,
        iterations: cli.iterations,
        payload_bytes: cli.payload_bytes,
        format: cli.format,
        seed: cli.seed,
        queue_depth,
        ingress: IngressConfig {
            mode: cli.ingress,
            bind_port: cli.ingress_port,
            connections: cli.ingress_connections,
            read_batch_size: cli.ingress_read_batch,
        },
        accelerator: AcceleratorConfig {
            mode: cli.accelerator,
            cuda_device: cli.cuda_device,
            gpu_batch_size: cli.gpu_batch_size,
        },
    })
}

/// Devuelve el nombre del allocator compilado activamente.
fn active_allocator_name() -> &'static str {
    if cfg!(feature = "use-jemalloc") {
        "jemalloc"
    } else if cfg!(feature = "use-mimalloc") {
        "mimalloc"
    } else {
        "system (glibc malloc)"
    }
}
