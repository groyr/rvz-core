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

fn md5_file(path: &str) -> String {
	let mut f = std::fs::File::open(path).expect("出力を開けません");
	let mut ctx = md5::Context::new();
	let mut buf = vec![0u8; 8 * 1024 * 1024];
	loop {
		let n = f.read(&mut buf).expect("読み込みに失敗");
		if n == 0 {
			break;
		}
		ctx.consume(&buf[..n]);
	}
	format!("{:x}", ctx.compute())
}

/// 吸い出し中で成長する ISO を末尾追従して読む（--follow 用）。
struct FollowReader {
	path: std::path::PathBuf,
	target: u64,
}

impl ReadAt for FollowReader {
	fn read_at(&self, offset: u64, len: usize) -> std::io::Result<Vec<u8>> {
		use std::time::{Duration, Instant};
		let want = (offset + len as u64).min(self.target);
		let mut last = 0u64;
		let mut last_change = Instant::now();
		loop {
			let size = std::fs::metadata(&self.path)?.len();
			if size >= want {
				break;
			}
			if size != last {
				last = size;
				last_change = Instant::now();
			}
			if last_change.elapsed() > Duration::from_secs(300) {
				return Err(std::io::Error::new(
					std::io::ErrorKind::TimedOut,
					"ISO の成長が止まりました",
				));
			}
			std::thread::sleep(Duration::from_millis(200));
		}
		let mut f = std::fs::File::open(&self.path)?;
		f.seek(SeekFrom::Start(offset))?;
		let n = (want - offset) as usize;
		let mut buf = vec![0u8; n];
		let mut got = 0usize;
		while got < n {
			let r = f.read(&mut buf[got..])?;
			if r == 0 {
				break;
			}
			got += r;
		}
		buf.truncate(got);
		Ok(buf)
	}
}

fn do_encode<R: ReadAt>(reader: &R, iso_size: u64, output: &str, level: i32, json: bool) {
	let out = std::fs::File::create(output).unwrap_or_else(|e| {
		eprintln!("作成できません: {}: {}", output, e);
		std::process::exit(1);
	});
	let mut writer = FileWriter { file: out };

	let t0 = std::time::Instant::now();
	let mut last_pct: i64 = -1;
	let wia_size = rvz::encode_iso_to_rvz(iso_size, level, reader, &mut writer, |done, total| {
		let pct = if total > 0 { (done.saturating_mul(100) / total) as i64 } else { 0 };
		if pct != last_pct {
			last_pct = pct;
			if json {
				println!("{{\"type\":\"progress\",\"value\":{:.4}}}", pct as f64 / 100.0);
			}
		}
	})
	.unwrap_or_else(|e| {
		if json {
			println!("{{\"type\":\"error\",\"message\":\"{}\"}}", e);
		}
		eprintln!("圧縮に失敗: {}", e);
		std::process::exit(1);
	});
	let elapsed_ms = t0.elapsed().as_millis();
	let md5hex = md5_file(output);
	if json {
		println!(
			"{{\"type\":\"done\",\"isoSize\":{},\"rvzSize\":{},\"md5\":\"{}\",\"elapsedMs\":{}}}",
			iso_size, wia_size, md5hex, elapsed_ms
		);
	} else {
		println!("iso_size={} rvz_size={} md5={}", iso_size, wia_size, md5hex);
	}
}

fn cmd_encode(
	input: &str,
	output: &str,
	level: i32,
	json: bool,
	follow: bool,
	target: Option<u64>,
) {
	if follow {
		use std::time::{Duration, Instant};
		let path = std::path::PathBuf::from(input);
		let wait_start = Instant::now();
		while !path.exists() {
			if wait_start.elapsed() > Duration::from_secs(600) {
				eprintln!("入力ファイルが現れません: {}", input);
				std::process::exit(1);
			}
			std::thread::sleep(Duration::from_millis(200));
		}
		let iso_size = target
			.unwrap_or_else(|| std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0));
		let reader = FollowReader { path, target: iso_size };
		do_encode(&reader, iso_size, output, level, json);
	} else {
		let file = std::fs::File::open(input).unwrap_or_else(|e| {
			eprintln!("開けません: {}: {}", input, e);
			std::process::exit(1);
		});
		let iso_size = file.metadata().unwrap().len();
		let reader = FileReader {
			file: Mutex::new(file),
		};
		do_encode(&reader, iso_size, output, level, json);
	}
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
				eprintln!(
					"使い方: rvz-cli encode <input.iso> <out.rvz> [level] [--json] [--follow --iso-size <bytes>]"
				);
				std::process::exit(2);
			}
			let level = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(3);
			let json = args.iter().any(|a| a == "--json");
			let follow = args.iter().any(|a| a == "--follow");
			let target = args
				.iter()
				.position(|a| a == "--iso-size")
				.and_then(|i| args.get(i + 1))
				.and_then(|s| s.parse().ok());
			cmd_encode(&args[2], &args[3], level, json, follow, target);
		}
		other => {
			eprintln!("不明なコマンド: {}", other);
			std::process::exit(2);
		}
	}
}
