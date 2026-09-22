//! rvz-core 縺ｮ讀懆ｨｼ逕ｨ CLI縲・//!
//! 菴ｿ縺・婿:
//!   rvz-cli decode <input.rvz>
//!
//! RVZ 繧貞ｱ暮幕縺励！SO 蜈ｨ菴薙・ MD5 縺ｨ繧ｵ繧､繧ｺ繧定｡ｨ遉ｺ縺吶ｋ・・SO 縺ｯ繝輔ぃ繧､繝ｫ縺ｸ譖ｸ縺榊・縺輔↑縺・ｼ峨・//! 蜿ら・ ISO 縺ｮ MD5 縺ｨ豈碑ｼ・☆繧九％縺ｨ縺ｧ繝・さ繝ｼ繝縺ｮ豁｣縺励＆繧呈､懆ｨｼ縺吶ｋ縲・
use std::io::{Read, Seek, SeekFrom};
use std::sync::Mutex;

use rvz::ReadAt;

struct FileReader {
	file: Mutex<std::fs::File>,
}

impl ReadAt for FileReader {
	fn read_at(&self, offset: u64, len: usize) -> std::io::Result<Vec<u8>> {
		let mut f = self.file.lock().unwrap();
		f.seek(SeekFrom::Start(offset))?;
		let mut buf = vec![0u8; len];
		let mut got = 0usize;
		while got < len {
			let n = f.read(&mut buf[got..])?;
			if n == 0 {
				break;
			}
			got += n;
		}
		buf.truncate(got);
		Ok(buf)
	}
}

struct Md5Sink {
	hasher: md5::Context,
	pos: u64,
	contiguous: bool,
	prefix: Vec<u8>,
}

impl Md5Sink {
	fn new() -> Self {
		Self {
			hasher: md5::Context::new(),
			pos: 0,
			contiguous: true,
			prefix: Vec::new(),
		}
	}
	fn write(&mut self, offset: u64, data: &[u8]) {
		if offset != self.pos {
			self.contiguous = false;
		}
		if self.prefix.len() < 64 {
			let need = 64 - self.prefix.len();
			self.prefix.extend_from_slice(&data[..need.min(data.len())]);
		}
		self.hasher.consume(data);
		self.pos += data.len() as u64;
	}
}

fn main() {
	let args: Vec<String> = std::env::args().collect();
	if args.len() < 3 || args[1] != "decode" {
		eprintln!("菴ｿ縺・婿: rvz-cli decode <input.rvz>");
		std::process::exit(2);
	}
	let path = &args[2];
	let file = std::fs::File::open(path).unwrap_or_else(|e| {
		eprintln!("髢九￠縺ｾ縺帙ｓ: {}: {}", path, e);
		std::process::exit(1);
	});
	let file_size = file.metadata().unwrap().len();
	let reader = FileReader {
		file: Mutex::new(file),
	};

	let mut sink = Md5Sink::new();
	let iso_size = rvz::decompress_rvz(file_size, &reader, |off, data| sink.write(off, data))
		.unwrap_or_else(|e| {
			eprintln!("螻暮幕縺ｫ螟ｱ謨・ {}", e);
			std::process::exit(1);
		});

	let pos = sink.pos;
	let contiguous = sink.contiguous;
	let digest = sink.hasher.compute();
	println!(
		"iso_size={} bytes_written={} contiguous={}",
		iso_size, pos, contiguous
	);
	println!("md5={:x}", digest);
	print!("prefix64=");
	for b in &sink.prefix {
		print!("{:02x}", b);
	}
	println!();
}
