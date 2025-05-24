#[macro_export]
macro_rules! tokio_main {
	($($fn:tt)*) => {
		fn main() -> anyhow::Result<()> {
			let rt = if args().nth(1).unwrap() == "mt" {
				tokio::runtime::Builder::new_multi_thread()
			} else {
				tokio::runtime::Builder::new_current_thread()
			}
			.enable_all()
			.build()?;

			rt.block_on(async { $($fn)* })
		}
	};
}
