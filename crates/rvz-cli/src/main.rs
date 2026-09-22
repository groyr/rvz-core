//! rvz-core の検証用 CLI。
//!
//! 使い方:
//!   rvz-cli decode <input.rvz>              RVZ を展開し ISO 全体の MD5 を表示
//!   rvz-cli encode <input.iso> <out.rvz> [level]   ISO を RVZ へ圧縮

use std::io::{Read, Seek, SeekFrom, Write};
use std::sync::Mutex;

use rvz::{ReadAt, WriteAt};

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

struct FileWriter {
	file: std::fs::File,
}

impl WriteAt for FileWriter {
	fn write_at(&mut self, offset: u64, data: &[u8]) -> std::io::Result<()> {
		self.file.seek(SeekFrom::Start(offset))?;
		self.file.write_all(data)
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

fn cmd_decode(path: &str) {
	let file = std::fs::File::open(path).unwrap_or_else(|e| {
		eprintln!("開けません: {}: {}", path, e);
		std::process::exit(1);
	});
	let file_size = file.metadata().unwrap().len();
	let reader = FileReader {
		file: Mutex::new(file),
	};

	let mut sink = Md5Sink::new();
	let iso_size = rvz::decompress_rvz(file_size, &reader, |off, data| sink.write(off, data))
		.unwrap_or_else(|e| {
			eprintln!("展開に失敗: {}", e);
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

fn cmd_decode_push(path: &str) {
	use rvz::PushDecoder;
	let file = std::fs::File::open(path).unwrap_or_else(|e| {
		eprintln!("開けません: {}: {}", path, e);
		std::process::exit(1);
	});
	let file_size = file.metadata().unwrap().len();
	let reader = FileReader {
		file: Mutex::new(file),
	};

	let mut dec = PushDecoder::new(file_size);
	let mut sink = Md5Sink::new();
	while let Some(req) = dec.request() {
		let bytes = if req.len == 0 {
			Vec::new()
		} else {
			reader.read_at(req.offset, req.len).unwrap()
		};
		if let Err(e) = dec.feed(&bytes) {
			eprintln!("展開に失敗: {}", e);
			std::process::exit(1);
		}
		while let Some(out) = dec.take_output() {
			sink.write(out.offset, &out.data);
		}
	}
	while let Some(out) = dec.take_output() {
		sink.write(out.offset, &out.data);
	}
	let iso_size = dec.output_size();
	let pos = sink.pos;
	let contiguous = sink.contiguous;
	let digest = sink.hasher.compute();
	println!(
		"iso_size={} bytes_written={} contiguous={}",
		iso_size, pos, contiguous
	);
	println!("md5={:x}", digest);
}

fn cmd_encode(input: &str, output: &str, level: i32) {
	let file = std::fs::File::open(input).unwrap_or_else(|e| {
		eprintln!("開けません: {}: {}", input, e);
		std::process::exit(1);
	});
	let iso_size = file.metadata().unwrap().len();
	let reader = FileReader {
		file: Mutex::new(file),
	};
	let out = std::fs::File::create(output).unwrap_or_else(|e| {
		eprintln!("作成できません: {}: {}", output, e);
		std::process::exit(1);
	});
	let mut writer = FileWriter { file: out };

	let wia_size =
		rvz::encode_iso_to_rvz(iso_size, level, &reader, &mut writer).unwrap_or_else(|e| {
			eprintln!("圧縮に失敗: {}", e);
			std::process::exit(1);
		});
	println!("iso_size={} rvz_size={}", iso_size, wia_size);
}

fn main() {
	let args: Vec<String> = std::env::args().collect();
	if args.len() < 3 {
		eprintln!("使い方: rvz-cli decode <input.rvz> | encode <input.iso> <out.rvz> [level]");
		std::process::exit(2);
	}
	match args[1].as_str() {
		"decode" => cmd_decode(&args[2]),
		"decode-push" => cmd_decode_push(&args[2]),
		"encode" => {
			if args.len() < 4 {
				eprintln!("使い方: rvz-cli encode <input.iso> <out.rvz> [level]");
				std::process::exit(2);
			}
			let level = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(3);
			cmd_encode(&args[2], &args[3], level);
		}
		other => {
			eprintln!("不明なコマンド: {}", other);
			std::process::exit(2);
		}
	}
}
