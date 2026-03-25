/// シンプルなMP4 Muxer実装
/// - HEVC (H.265) のみ対応
/// - fast_start無効（moovを末尾に書く）
/// - 音声なし
/// - 外部クレート不要（std only）
/// - Write + Seek を実装した任意のWriterに対応
use std::io::{self, SeekFrom};
use tokio::io::{AsyncSeekExt, AsyncWriteExt};

// ============================================================
// 公開API
// ============================================================

pub struct Mp4Muxer<W: AsyncWriteExt + AsyncSeekExt + Unpin> {
    writer: W,
    width: u32,
    height: u32,
    timescale: u32, // トラックのタイムスケール（例: 90000）
    samples: Vec<Sample>,
    mdat_start: u64, // mdatのデータ開始位置
    bytes_written: u64,
    vps: Vec<u8>,
    sps: Vec<u8>,
    pps: Vec<u8>,
    param_sets_extracted: bool,
}

struct Sample {
    offset: u64, // ファイル内のオフセット
    size: u32,
    duration: u32, // タイムスケール単位
    is_key: bool,
}

impl<W: AsyncWriteExt + AsyncSeekExt + Unpin> Mp4Muxer<W> {
    /// 新しいMuxerを作成する
    /// timescale: 通常90000（90kHz）。ptsはこの単位で渡す
    pub async fn new(mut writer: W, width: u32, height: u32, timescale: u32) -> io::Result<Self> {
        // ftyp ボックスを先頭に書く
        write_ftyp(&mut writer).await?;

        // size=1 はlargesize形式を示す
        writer.write_all(&1u32.to_be_bytes()).await?;
        writer.write_all(b"mdat").await?;
        // largesize（64bit）: 後で上書きするのでとりあえず0
        writer.write_all(&0u64.to_be_bytes()).await?;

        let mdat_start = writer.stream_position().await?;

        Ok(Self {
            writer,
            width,
            height,
            timescale,
            samples: Vec::new(),
            mdat_start,
            bytes_written: 0,
            vps: Vec::new(),
            sps: Vec::new(),
            pps: Vec::new(),
            param_sets_extracted: false,
        })
    }

    /// HEVCフレームを書き込む
    ///
    /// data: Annex B形式（00 00 00 01 スタートコード付き）
    /// pts: タイムスケール単位のプレゼンテーションタイムスタンプ
    /// duration: タイムスケール単位のフレーム時間（25fpsなら90000/25=3600）
    /// is_key: IDRフレームならtrue
    pub async fn write_video(
        &mut self,
        data: &[u8],
        duration: u32,
        is_key: bool,
    ) -> io::Result<()> {
        // 最初のキーフレームからVPS/SPS/PPSを抽出
        if !self.param_sets_extracted && is_key {
            extract_hevc_param_sets(data, &mut self.vps, &mut self.sps, &mut self.pps);
            self.param_sets_extracted = true;
        }

        // Annex B → AVCC/HVC1形式（4バイト長さプレフィックス）に変換して書き込む
        let offset = self.mdat_start + self.bytes_written;
        let mut frame_size = 0u32;

        for nal in AnnexBIter::new(data) {
            if nal.is_empty() {
                continue;
            }
            let nal_len = nal.len() as u32;
            self.writer.write_all(&nal_len.to_be_bytes()).await?;
            self.writer.write_all(nal).await?;
            frame_size += 4 + nal_len;
        }

        self.bytes_written += frame_size as u64;
        self.samples.push(Sample {
            offset,
            size: frame_size,
            duration,
            is_key,
        });

        Ok(())
    }

    /// 録画を終了してmoovを書き込む
    pub async fn finish(mut self) -> io::Result<()> {
        // mdat の largesize を上書き
        // largesize = ヘッダ(4+4+8=16バイト) + データ
        let mdat_total = 16u64 + self.bytes_written;
        let mdat_header_pos = self.mdat_start - 16; // size(4) + "mdat"(4) + largesize(8)
        self.writer.seek(SeekFrom::Start(mdat_header_pos)).await?;
        self.writer.write_all(&1u32.to_be_bytes()).await?; // size=1 → largesize形式
        self.writer.write_all(b"mdat").await?;
        self.writer.write_all(&mdat_total.to_be_bytes()).await?;

        // moov を末尾に書く
        self.writer
            .seek(SeekFrom::Start(self.mdat_start + self.bytes_written))
            .await?;
        let moov = self.build_moov();
        self.writer.write_all(&moov).await?;
        self.writer.flush().await?;

        Ok(())
    }

    // ============================================================
    // moovボックス構築
    // ============================================================

    fn build_moov(&self) -> Vec<u8> {
        let mut buf = Vec::new();

        // mvhd
        buf.extend(build_mvhd(self.total_duration(), self.timescale));

        // trak（videoトラック）
        buf.extend(self.build_trak());

        box_wrap(b"moov", &buf)
    }

    fn total_duration(&self) -> u32 {
        self.samples.iter().map(|s| s.duration).sum()
    }

    fn build_trak(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend(build_tkhd(self.total_duration(), self.width, self.height));
        buf.extend(self.build_mdia());
        box_wrap(b"trak", &buf)
    }

    fn build_mdia(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend(build_mdhd(self.total_duration(), self.timescale));
        buf.extend(build_hdlr());
        buf.extend(self.build_minf());
        box_wrap(b"mdia", &buf)
    }

    fn build_minf(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend(build_vmhd());
        buf.extend(build_dinf());
        buf.extend(self.build_stbl());
        box_wrap(b"minf", &buf)
    }

    fn build_stbl(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend(self.build_stsd());
        buf.extend(build_stts(&self.samples));
        buf.extend(build_stss(&self.samples));
        buf.extend(build_stsc());
        buf.extend(build_stsz(&self.samples));
        buf.extend(build_stco(&self.samples));
        box_wrap(b"stbl", &buf)
    }

    fn build_stsd(&self) -> Vec<u8> {
        // hvc1 サンプルエントリ
        let hvc1 = self.build_hvc1();

        // stsd: version(1) + flags(3) + entry_count(4) + entries
        let mut buf = vec![0u8; 4]; // version + flags
        buf.extend(&1u32.to_be_bytes()); // entry_count = 1
        buf.extend(&hvc1);
        box_wrap(b"stsd", &buf)
    }

    fn build_hvc1(&self) -> Vec<u8> {
        let mut buf = Vec::new();

        // reserved(6) + data_reference_index(2)
        buf.extend(&[0u8; 6]);
        buf.extend(&1u16.to_be_bytes());

        // pre_defined(2) + reserved(2) + pre_defined(12)
        buf.extend(&[0u8; 16]);

        // width, height
        buf.extend(&(self.width as u16).to_be_bytes());
        buf.extend(&(self.height as u16).to_be_bytes());

        // horiz/vert resolution: 72 dpi (0x00480000)
        buf.extend(&0x00480000u32.to_be_bytes());
        buf.extend(&0x00480000u32.to_be_bytes());

        // reserved(4) + frame_count(2) + compressorname(32) + depth(2) + pre_defined(2)
        buf.extend(&[0u8; 4]);
        buf.extend(&1u16.to_be_bytes()); // frame_count
        buf.extend(&[0u8; 32]); // compressorname
        buf.extend(&0x0018u16.to_be_bytes()); // depth
        buf.extend(&0xFFFFu16.to_be_bytes()); // pre_defined

        // hvcC ボックス
        buf.extend(self.build_hvcc());

        box_wrap(b"hvc1", &buf)
    }

    fn build_hvcc(&self) -> Vec<u8> {
        // HEVCDecoderConfigurationRecord
        let mut record = Vec::new();

        // configurationVersion = 1
        record.push(1u8);

        // profile_space(2bit) | tier_flag(1bit) | profile_idc(5bit)
        // general_profile_compatibility_flags(32bit)
        // general_constraint_indicator_flags(48bit)
        // general_level_idc(8bit)
        // SPS から正確に取るのが理想だが、
        // Main profile / Level 4.1 (組み込みHWエンコーダの典型値) で固定
        record.push(0x01); // profile_space=0, tier=0, profile_idc=1(Main)
        record.extend(&0x60000000u32.to_be_bytes()); // general_profile_compatibility
        record.extend(&[0x90, 0x00, 0x00, 0x00, 0x00, 0x00]); // constraint flags
        record.push(0x00); // reserved
        record.extend(&[0xFF, 0xFF]); // min_spatial_segmentation_idc
        record.push(0xFC); // parallelismType
        record.push(0xFD); // chroma_format_idc = 1 (4:2:0)
        record.push(0xF8); // bit_depth_luma - 8
        record.push(0xF8); // bit_depth_chroma - 8
        record.extend(&0x0000u16.to_be_bytes()); // avgFrameRate
        // constantFrameRate(2) | numTemporalLayers(3) | temporalIdNested(1) | lengthSizeMinusOne(2)
        record.push(0x0F); // lengthSizeMinusOne=3 → 4バイト長さプレフィックス

        // numOfArrays
        let mut arrays: Vec<(u8, &[u8])> = Vec::new();
        if !self.vps.is_empty() {
            arrays.push((0x20, &self.vps));
        } // VPS
        if !self.sps.is_empty() {
            arrays.push((0x21, &self.sps));
        } // SPS
        if !self.pps.is_empty() {
            arrays.push((0x22, &self.pps));
        } // PPS

        record.push(arrays.len() as u8);

        for (nal_type, nal_data) in &arrays {
            // array_completeness(1) | reserved(1) | NAL_unit_type(6)
            record.push(0x80 | nal_type); // array_completeness=1
            record.extend(&1u16.to_be_bytes()); // numNalus = 1
            record.extend(&(nal_data.len() as u16).to_be_bytes());
            record.extend(*nal_data);
        }

        box_wrap(b"hvcC", &record)
    }
}

// ============================================================
// サンプルテーブルボックス群
// ============================================================

/// stts: サンプル時間テーブル（連続する同一durationをランレングス圧縮）
fn build_stts(samples: &[Sample]) -> Vec<u8> {
    let mut entries: Vec<(u32, u32)> = Vec::new(); // (count, duration)
    for s in samples {
        if let Some(last) = entries.last_mut()
            && last.1 == s.duration
        {
            last.0 += 1;
            continue;
        }
        entries.push((1, s.duration));
    }

    let mut buf = vec![0u8; 4]; // version + flags
    buf.extend(&(entries.len() as u32).to_be_bytes());
    for (count, duration) in entries {
        buf.extend(&count.to_be_bytes());
        buf.extend(&duration.to_be_bytes());
    }
    box_wrap(b"stts", &buf)
}

/// stss: キーフレームのサンプル番号テーブル（1-indexed）
fn build_stss(samples: &[Sample]) -> Vec<u8> {
    let keyframes: Vec<u32> = samples
        .iter()
        .enumerate()
        .filter(|(_, s)| s.is_key)
        .map(|(i, _)| (i + 1) as u32)
        .collect();

    let mut buf = vec![0u8; 4]; // version + flags
    buf.extend(&(keyframes.len() as u32).to_be_bytes());
    for idx in keyframes {
        buf.extend(&idx.to_be_bytes());
    }
    box_wrap(b"stss", &buf)
}

/// stsc: サンプル→チャンク対応テーブル（全サンプルを1チャンク1サンプルで記録）
fn build_stsc() -> Vec<u8> {
    // シンプルに: 全チャンク = サンプル1個ずつ
    // first_chunk=1, samples_per_chunk=1, sample_description_index=1
    let mut buf = vec![0u8; 4]; // version + flags
    buf.extend(&1u32.to_be_bytes()); // entry_count = 1
    buf.extend(&1u32.to_be_bytes()); // first_chunk
    buf.extend(&1u32.to_be_bytes()); // samples_per_chunk
    buf.extend(&1u32.to_be_bytes()); // sample_description_index
    box_wrap(b"stsc", &buf)
}

/// stsz: サンプルサイズテーブル
fn build_stsz(samples: &[Sample]) -> Vec<u8> {
    let mut buf = vec![0u8; 4]; // version + flags
    buf.extend(&0u32.to_be_bytes()); // sample_size = 0 (可変長)
    buf.extend(&(samples.len() as u32).to_be_bytes());
    for s in samples {
        buf.extend(&s.size.to_be_bytes());
    }
    box_wrap(b"stsz", &buf)
}

/// stco: チャンクオフセットテーブル（各サンプルのファイル内オフセット）
fn build_stco(samples: &[Sample]) -> Vec<u8> {
    // オフセットが32bitに収まるか確認し、必要なら co64 を使う
    let needs_co64 = samples.iter().any(|s| s.offset > u32::MAX as u64);

    let mut buf = vec![0u8; 4]; // version + flags
    buf.extend(&(samples.len() as u32).to_be_bytes());

    if needs_co64 {
        for s in samples {
            buf.extend(&s.offset.to_be_bytes());
        }
        box_wrap(b"co64", &buf)
    } else {
        for s in samples {
            buf.extend(&(s.offset as u32).to_be_bytes());
        }
        box_wrap(b"stco", &buf)
    }
}

// ============================================================
// 固定ボックス群
// ============================================================

async fn write_ftyp<W: AsyncWriteExt + Unpin>(w: &mut W) -> io::Result<()> {
    let mut buf = Vec::new();
    buf.extend(b"hvc1"); // major_brand
    buf.extend(&0u32.to_be_bytes()); // minor_version
    buf.extend(b"hvc1");
    buf.extend(b"iso4");
    buf.extend(b"mp41");
    w.write_all(&box_wrap(b"ftyp", &buf)).await
}

fn build_mvhd(duration: u32, timescale: u32) -> Vec<u8> {
    let mut buf = vec![0u8; 4]; // version(0) + flags
    buf.extend(&0u32.to_be_bytes()); // creation_time
    buf.extend(&0u32.to_be_bytes()); // modification_time
    buf.extend(&timescale.to_be_bytes());
    buf.extend(&duration.to_be_bytes());
    buf.extend(&0x00010000u32.to_be_bytes()); // rate = 1.0
    buf.extend(&0x0100u16.to_be_bytes()); // volume = 1.0
    buf.extend(&[0u8; 10]); // reserved
    // unity matrix
    buf.extend(&[
        0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x40, 0x00, 0x00, 0x00,
    ]);
    buf.extend(&[0u8; 24]); // pre_defined
    buf.extend(&0xFFFFFFFFu32.to_be_bytes()); // next_track_ID
    box_wrap(b"mvhd", &buf)
}

fn build_tkhd(duration: u32, width: u32, height: u32) -> Vec<u8> {
    // flags: track_enabled(1) | track_in_movie(2) | track_in_preview(4) = 3
    let mut buf = vec![0x00, 0x00, 0x00, 0x03]; // version + flags
    buf.extend(&0u32.to_be_bytes()); // creation_time
    buf.extend(&0u32.to_be_bytes()); // modification_time
    buf.extend(&1u32.to_be_bytes()); // track_ID = 1
    buf.extend(&[0u8; 4]); // reserved
    buf.extend(&duration.to_be_bytes());
    buf.extend(&[0u8; 8]); // reserved
    buf.extend(&0u16.to_be_bytes()); // layer
    buf.extend(&0u16.to_be_bytes()); // alternate_group
    buf.extend(&0u16.to_be_bytes()); // volume (video=0)
    buf.extend(&[0u8; 2]); // reserved
    // unity matrix
    buf.extend(&[
        0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x40, 0x00, 0x00, 0x00,
    ]);
    // width/height は 16.16 fixed point
    buf.extend(&(width << 16).to_be_bytes());
    buf.extend(&(height << 16).to_be_bytes());
    box_wrap(b"tkhd", &buf)
}

fn build_mdhd(duration: u32, timescale: u32) -> Vec<u8> {
    let mut buf = vec![0u8; 4]; // version + flags
    buf.extend(&0u32.to_be_bytes()); // creation_time
    buf.extend(&0u32.to_be_bytes()); // modification_time
    buf.extend(&timescale.to_be_bytes());
    buf.extend(&duration.to_be_bytes());
    buf.extend(&0x55C4u16.to_be_bytes()); // language: "und"
    buf.extend(&0u16.to_be_bytes()); // pre_defined
    box_wrap(b"mdhd", &buf)
}

fn build_hdlr() -> Vec<u8> {
    let mut buf = vec![0u8; 4]; // version + flags
    buf.extend(&0u32.to_be_bytes()); // pre_defined
    buf.extend(b"vide"); // handler_type
    buf.extend(&[0u8; 12]); // reserved
    buf.extend(b"VideoHandler\0");
    box_wrap(b"hdlr", &buf)
}

fn build_vmhd() -> Vec<u8> {
    let mut buf = vec![0x00, 0x00, 0x00, 0x01]; // version + flags (flags=1 per spec)
    buf.extend(&[0u8; 4]); // graphicsMode + opcolor
    box_wrap(b"vmhd", &buf)
}

fn build_dinf() -> Vec<u8> {
    // dref: データが同一ファイル内にあることを示す
    let mut dref_buf = vec![0u8; 4]; // version + flags
    dref_buf.extend(&1u32.to_be_bytes()); // entry_count
    // "url " ボックス: flags=1 (self-contained)
    let url = box_wrap_with_fullheader(b"url ", 0, 1, &[]);
    dref_buf.extend(&url);

    let dref = box_wrap(b"dref", &dref_buf);
    box_wrap(b"dinf", &dref)
}

// ============================================================
// ユーティリティ
// ============================================================

/// ボックスをラップする（サイズ + 4文字タイプ + データ）
fn box_wrap(box_type: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let size = (8 + data.len()) as u32;
    let mut buf = Vec::with_capacity(size as usize);
    buf.extend(&size.to_be_bytes());
    buf.extend(box_type);
    buf.extend(data);
    buf
}

/// fullbox形式（version + flags付き）のボックスをラップする
fn box_wrap_with_fullheader(box_type: &[u8; 4], version: u8, flags: u32, data: &[u8]) -> Vec<u8> {
    let mut buf = Vec::new();
    buf.push(version);
    buf.extend(&flags.to_be_bytes()[1..]); // 3バイトのflags
    buf.extend(data);
    box_wrap(box_type, &buf)
}

/// Annex B ストリームからHEVCパラメータセット（VPS/SPS/PPS）を抽出する
fn extract_hevc_param_sets(data: &[u8], vps: &mut Vec<u8>, sps: &mut Vec<u8>, pps: &mut Vec<u8>) {
    for nal in AnnexBIter::new(data) {
        if nal.len() < 2 {
            continue;
        }
        let nal_type = (nal[0] >> 1) & 0x3F;
        match nal_type {
            32 => {
                *vps = nal.to_vec();
            } // VPS
            33 => {
                *sps = nal.to_vec();
            } // SPS
            34 => {
                *pps = nal.to_vec();
            } // PPS
            _ => {}
        }
    }
}

/// Annex B（スタートコード区切り）のNALユニットイテレータ
struct AnnexBIter<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> AnnexBIter<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    /// 次のスタートコード位置を探す
    fn find_next_start(&self, from: usize) -> Option<usize> {
        let d = self.data;
        let mut i = from;
        while i + 2 < d.len() {
            if d[i] == 0 && d[i + 1] == 0 {
                if d[i + 2] == 1 {
                    return Some(i);
                }
                if i + 3 < d.len() && d[i + 2] == 0 && d[i + 3] == 1 {
                    return Some(i);
                }
            }
            i += 1;
        }
        None
    }
}

impl<'a> Iterator for AnnexBIter<'a> {
    type Item = &'a [u8];

    fn next(&mut self) -> Option<Self::Item> {
        let d = self.data;

        // 現在位置からスタートコードを探す
        let start_code_pos = self.find_next_start(self.pos)?;

        // スタートコードのバイト数（3バイトか4バイト）
        let sc_len = if d[start_code_pos + 2] == 1 { 3 } else { 4 };
        let nal_start = start_code_pos + sc_len;

        // 次のスタートコード or 末尾までがNALデータ
        let nal_end = self.find_next_start(nal_start).unwrap_or(d.len());

        self.pos = nal_end;

        let nal = &d[nal_start..nal_end];
        if nal.is_empty() {
            return self.next();
        }
        Some(nal)
    }
}
