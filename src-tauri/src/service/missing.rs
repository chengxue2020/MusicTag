// MusicTag — 缺失字段只读扫描。
//
// 这里仅读取选中的标签维度、图片存在性和歌词来源存在性；不调用 reader 的完整
// Song 读取，不编码封面，也不读取/写入 sidecar 内容。
//
// 例外：年份维度复用 reader::tagged_file_year 这一「年份读侧唯一出口」，
// 保证筛选判定与编辑表单读到的年份是同一组 key、同一套 WAV 回退规则。

use crate::model::{MissingField, MissingScanError, MissingScanResult, MissingSong};
use crate::service::lyrics::sidecar_lrc_path;
use crate::service::meta::is_audio_file;
use crate::service::reader;
use lofty::prelude::TaggedFileExt;
use lofty::probe::Probe;
use lofty::tag::{ItemKey, ItemValue};
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

/// 业务检查顺序固定；`Year` 追加在末尾，既有五维的顺序/索引保持不变。
const FIELD_ORDER: [MissingField; 6] = [
    MissingField::Title,
    MissingField::Artist,
    MissingField::Album,
    MissingField::Cover,
    MissingField::Lyrics,
    MissingField::Year,
];

/// 文件扫描线程的硬上限：即便机器有很多核，也最多同时开 4 个 worker。
/// 目录遍历始终单线程，只有「逐文件解析标签」这一段并行，避免同时打开过多
/// 音频文件把磁盘 I/O 与内存占满。
pub const MAX_MISSING_SCAN_WORKERS: usize = 4;

type ScanOutcome = Result<Option<MissingSong>, String>;

/// 计算本次实际启用的 worker 数：固定上限、CPU 可用并行度、待扫描文件数三者取最小。
/// 返回 0 或 1 时调用方直接走串行分支，不为小目录付出建线程的开销。
pub fn missing_scan_worker_count(file_count: usize) -> usize {
    let available = std::thread::available_parallelism()
        .map(|parallelism| parallelism.get())
        .unwrap_or(1);
    file_count.min(available).min(MAX_MISSING_SCAN_WORKERS)
}

/// 扫描目录中选定维度缺失的音频文件（自适应 worker 数）。
///
/// `checks` 为空表示未启用筛选，直接返回空结果。目录遍历和单文件解析错误分别
/// 以 command 可继续处理的 `errors` 返回；只有 worker 无法启动/汇合时才由 command
/// 转成 command-level 错误。
///
/// 目录遍历保持串行以固定路径顺序，逐文件标签解析按 `missing_scan_worker_count`
/// 得出的有限 worker 数并行；结果先按原下标归位再汇总，因此 `songs` / `errors`
/// 的顺序与串行实现一致，用户看到的列表不会因为机器核数不同而变化。
pub fn scan_missing(dir: &Path, checks: &[MissingField]) -> Result<MissingScanResult, String> {
    scan_missing_with_workers(dir, checks, None)
}

/// 与 `scan_missing` 相同，但允许显式指定 worker 上限；`None` 表示按 CPU 可用
/// 并行度与文件数自适应。`worker_limit` 仍会被 `MAX_MISSING_SCAN_WORKERS` 和
/// 待扫描文件数夹住，调用方无法绕过资源上限。
pub fn scan_missing_with_workers(
    dir: &Path,
    checks: &[MissingField],
    worker_limit: Option<usize>,
) -> Result<MissingScanResult, String> {
    if checks.is_empty() {
        return Ok(MissingScanResult {
            songs: Vec::new(),
            errors: Vec::new(),
        });
    }

    if !dir.is_dir() {
        return Err(format!("目录不存在或不可读取: {}", dir.to_string_lossy()));
    }

    let selected: Vec<MissingField> = FIELD_ORDER
        .into_iter()
        .filter(|field| checks.contains(field))
        .collect();

    // 只在这里遍历目录：并发遍历会放大随机 I/O，且让结果顺序不可预期。
    let mut errors = Vec::new();
    let mut paths: Vec<PathBuf> = Vec::new();
    for entry in WalkDir::new(dir).into_iter() {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                errors.push(MissingScanError {
                    path: error
                        .path()
                        .map(|path| path.to_string_lossy().into_owned())
                        .unwrap_or_else(|| dir.to_string_lossy().into_owned()),
                    reason: error.to_string(),
                });
                continue;
            }
        };

        if !entry.file_type().is_file() || !is_audio_file(entry.path()) {
            continue;
        }
        paths.push(entry.path().to_path_buf());
    }

    let worker_count = worker_limit
        .unwrap_or_else(|| missing_scan_worker_count(paths.len()))
        .min(paths.len())
        .min(MAX_MISSING_SCAN_WORKERS);
    let outcomes: Vec<ScanOutcome> = if worker_count <= 1 {
        // 空目录、单文件或单核：不建线程，直接复用串行的单文件判定。
        paths
            .iter()
            .map(|path| scan_file(path, &selected))
            .collect()
    } else {
        scan_files_in_parallel(&paths, &selected, worker_count)?
    };

    let mut songs = Vec::new();
    for (path, outcome) in paths.iter().zip(outcomes) {
        match outcome {
            Ok(Some(song)) => songs.push(song),
            Ok(None) => {}
            Err(reason) => errors.push(MissingScanError {
                path: path.to_string_lossy().into_owned(),
                reason,
            }),
        }
    }

    Ok(MissingScanResult { songs, errors })
}

/// 用固定数量的 worker 交错分摊文件解析（worker `i` 处理下标 `i, i+n, ...`），
/// 让不同目录下耗时不同的文件更均衡。返回的 `ScanOutcome` 按输入下标排序，
/// 与 `paths` 一一对应。零新增依赖，只借用 `std::thread::scope`。
fn scan_files_in_parallel(
    paths: &[PathBuf],
    selected: &[MissingField],
    worker_count: usize,
) -> Result<Vec<ScanOutcome>, String> {
    std::thread::scope(|scope| -> Result<Vec<ScanOutcome>, String> {
        let mut handles = Vec::with_capacity(worker_count);
        for worker_index in 0..worker_count {
            handles.push(scope.spawn(move || {
                let mut partial = Vec::new();
                let mut index = worker_index;
                while index < paths.len() {
                    partial.push((index, scan_file(paths[index].as_path(), selected)));
                    index += worker_count;
                }
                partial
            }));
        }

        let mut indexed: Vec<(usize, ScanOutcome)> = Vec::with_capacity(paths.len());
        for handle in handles {
            let partial = handle
                .join()
                .map_err(|_| "缺失字段扫描 worker 异常终止".to_string())?;
            indexed.extend(partial);
        }
        indexed.sort_by_key(|item| item.0);
        Ok(indexed.into_iter().map(|(_, outcome)| outcome).collect())
    })
}

fn scan_file(path: &Path, selected: &[MissingField]) -> Result<Option<MissingSong>, String> {
    let tagged_file = Probe::open(path)
        .and_then(|probed| probed.read())
        .map_err(|error| format!("读取标签失败: {error}"))?;
    let tag = tagged_file.primary_tag();

    let mut missing = Vec::new();
    for field in selected {
        let is_missing = match field {
            MissingField::Title => text_missing(tag, ItemKey::TrackTitle),
            MissingField::Artist => text_missing(tag, ItemKey::TrackArtist),
            MissingField::Album => text_missing(tag, ItemKey::AlbumTitle),
            MissingField::Cover => tag.map(|tag| tag.pictures().is_empty()).unwrap_or(true),
            MissingField::Lyrics => lyrics_missing(tag, path),
            // 年份必须走 reader 的同一出口：`RecordingDate` 优先、`Year` 兜底，
            // WAV 再回退 RIFF INFO。空白串按其他文本维度一样 trim 后视为缺失。
            MissingField::Year => reader::tagged_file_year(&tagged_file).trim().is_empty(),
        };
        if is_missing {
            missing.push(*field);
        }
    }

    if missing.is_empty() {
        Ok(None)
    } else {
        Ok(Some(MissingSong {
            path: path.to_string_lossy().into_owned(),
            missing,
        }))
    }
}

fn text_missing(tag: Option<&lofty::tag::Tag>, key: ItemKey) -> bool {
    tag.and_then(|tag| tag.get_string(key))
        .map(|value| value.trim().is_empty())
        .unwrap_or(true)
}

fn lyrics_missing(tag: Option<&lofty::tag::Tag>, audio_path: &Path) -> bool {
    let has_embedded = tag
        .map(|tag| {
            tag.items()
                .filter(|item| matches!(item.key(), ItemKey::Lyrics | ItemKey::UnsyncLyrics))
                .any(|item| match item.value() {
                    ItemValue::Text(value) | ItemValue::Locator(value) => !value.trim().is_empty(),
                    ItemValue::Binary(value) => !value.is_empty(),
                })
        })
        .unwrap_or(false);

    if has_embedded {
        return false;
    }

    !sidecar_lrc_path(audio_path).is_file()
}
