//! What a Flatpak bundle installs, read from its header: the app's ref and
//! the version its AppStream data names. A bundle is one GVariant, an OSTree
//! static delta whose first member, an `a{sv}`, holds `ref` and `appdata`
//! next to the inline delta parts. Only those two entries are read, so even
//! a large bundle costs a few small reads.
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// Entries the header may have; real bundles have about ten.
const MAX_ENTRIES: u64 = 4096;
const MAX_KEY: u64 = 256;
const MAX_REF: u64 = 1024;
/// The compressed AppStream data, and the most it may grow to.
const MAX_APPDATA: u64 = 4 * 1024 * 1024;
const MAX_APPDATA_XML: u64 = 16 * 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Bundle {
    /// The app's ID, such as `org.example.App`.
    pub id: String,
    pub arch: String,
    pub branch: String,
    /// The newest release the bundle's AppStream data names, if any.
    pub version: Option<String>,
}

pub fn read(path: &Path) -> Result<Bundle, String> {
    let mut file = std::fs::File::open(path).map_err(|error| error.to_string())?;
    let len = file.metadata().map_err(|error| error.to_string())?.len();
    read_from(&mut file, len)
}

/// How many bytes each framing offset of a `len`-byte container takes.
fn offset_size(len: u64) -> u64 {
    match len {
        0 => 0,
        1..=0xff => 1,
        0x100..=0xffff => 2,
        0x1_0000..=0xffff_ffff => 4,
        _ => 8,
    }
}

fn align8(at: u64) -> u64 {
    at.div_ceil(8) * 8
}

fn read_at(reader: &mut (impl Read + Seek), at: u64, len: u64) -> Result<Vec<u8>, String> {
    let mut bytes = vec![0; usize::try_from(len).map_err(|_| malformed())?];
    reader
        .seek(SeekFrom::Start(at))
        .and_then(|_| reader.read_exact(&mut bytes))
        .map_err(|_| malformed())?;
    Ok(bytes)
}

fn offset_at(reader: &mut (impl Read + Seek), at: u64, size: u64) -> Result<u64, String> {
    let bytes = read_at(reader, at, size)?;
    Ok(bytes
        .iter()
        .rev()
        .fold(0, |value, byte| value << 8 | u64::from(*byte)))
}

fn malformed() -> String {
    "not a Flatpak bundle".into()
}

pub(crate) fn read_from(reader: &mut (impl Read + Seek), len: u64) -> Result<Bundle, String> {
    let size = offset_size(len);
    if len <= size {
        return Err(malformed());
    }
    // The last framing offset of the whole bundle ends its first member.
    let dict_end = offset_at(reader, len - size, size)?;
    let size = offset_size(dict_end);
    if dict_end <= size || dict_end > len - offset_size(len) {
        return Err(malformed());
    }
    // An array's last offset ends its last entry, where its offsets start.
    let table = offset_at(reader, dict_end - size, size)?;
    if table >= dict_end
        || !(dict_end - table).is_multiple_of(size)
        || (dict_end - table) / size > MAX_ENTRIES
    {
        return Err(malformed());
    }
    let offsets = read_at(reader, table, dict_end - table)?;
    let (mut reference, mut appdata) = (None, None);
    let mut start = 0;
    for offset in offsets.chunks(size as usize) {
        let end = offset
            .iter()
            .rev()
            .fold(0, |value, byte| value << 8 | u64::from(*byte));
        let entry = align8(start);
        if end < entry || end > table {
            return Err(malformed());
        }
        let (key, value) = entry_value(reader, entry, end - entry)?;
        match key.as_slice() {
            b"ref" => reference = value.filter(|(ty, _)| ty == b"s").map(|(_, body)| body),
            b"appdata" => appdata = value.filter(|(ty, _)| ty == b"ay").map(|(_, body)| body),
            _ => {}
        }
        start = end;
    }
    let reference = reference.ok_or_else(|| "the bundle names no app".to_owned())?;
    let reference = reference
        .strip_suffix(b"\0")
        .and_then(|text| std::str::from_utf8(text).ok())
        .ok_or_else(malformed)?;
    let [kind, id, arch, branch] = reference.split('/').collect::<Vec<_>>()[..] else {
        return Err(malformed());
    };
    let valid = |part: &str| {
        !part.is_empty()
            && part
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
    };
    if !matches!(kind, "app" | "runtime") || ![id, arch, branch].into_iter().all(valid) {
        return Err(malformed());
    }
    Ok(Bundle {
        id: id.into(),
        arch: arch.into(),
        branch: branch.into(),
        version: appdata.as_deref().and_then(release_version),
    })
}

/// The key of the `{sv}` entry at `entry`, and for `ref` and `appdata` its
/// value's type and bytes. Other values, such as the delta parts, are not
/// read.
#[allow(clippy::type_complexity)]
fn entry_value(
    reader: &mut (impl Read + Seek),
    entry: u64,
    len: u64,
) -> Result<(Vec<u8>, Option<(Vec<u8>, Vec<u8>)>), String> {
    let size = offset_size(len);
    if len <= size {
        return Err(malformed());
    }
    let body = len - size;
    let key_end = offset_at(reader, entry + body, size)?;
    if key_end == 0 || key_end > body {
        return Err(malformed());
    }
    if key_end > MAX_KEY {
        return Ok((vec![], None));
    }
    let mut key = read_at(reader, entry, key_end)?;
    if key.pop() != Some(0) {
        return Err(malformed());
    }
    let limit = match key.as_slice() {
        b"ref" => MAX_REF,
        b"appdata" => MAX_APPDATA,
        _ => return Ok((key, None)),
    };
    let value = align8(key_end);
    if value > body || body - value > limit + 16 {
        return Ok((key, None));
    }
    // A variant is its value, a zero byte, then its type.
    let mut variant = read_at(reader, entry + value, body - value)?;
    let Some(separator) = variant.iter().rposition(|b| *b == 0) else {
        return Err(malformed());
    };
    let ty = variant.split_off(separator + 1);
    variant.pop();
    Ok((key, Some((ty, variant))))
}

/// The version of the first `<release>` in gzipped AppStream XML, which
/// lists the newest first.
fn release_version(gzipped: &[u8]) -> Option<String> {
    let mut xml = String::new();
    flate2::read::GzDecoder::new(gzipped)
        .take(MAX_APPDATA_XML)
        .read_to_string(&mut xml)
        .ok()?;
    let mut rest = xml.as_str();
    let tag = loop {
        let at = rest.find("<release")?;
        rest = &rest[at + "<release".len()..];
        if rest.starts_with(|c: char| c.is_whitespace() || c == '>' || c == '/') {
            break &rest[..rest.find('>')?];
        }
    };
    let mut words = tag;
    while let Some(at) = words.find("version") {
        let before = &words[..at];
        words = &words[at + "version".len()..];
        if !before.ends_with(char::is_whitespace) {
            continue;
        }
        let value = words.trim_start().strip_prefix('=')?.trim_start();
        let quote = value.chars().next().filter(|c| *c == '"' || *c == '\'')?;
        let value = &value[1..];
        let version = &value[..value.find(quote)?];
        return (!version.is_empty()
            && version
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b".+-_~:".contains(&b)))
        .then(|| version.to_owned());
    }
    None
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::{Cursor, Write};

    /// `body` followed by framing offsets that make it `offsets.len()`
    /// offsets long, each as wide as the whole container needs.
    fn framed(body: &[u8], offsets: &[u64]) -> Vec<u8> {
        let size = [1, 2, 4, 8]
            .into_iter()
            .find(|size| offset_size(body.len() as u64 + size * offsets.len() as u64) == *size)
            .unwrap();
        let mut out = body.to_vec();
        for offset in offsets {
            out.extend_from_slice(&offset.to_le_bytes()[..size as usize]);
        }
        out
    }

    fn pad(bytes: &mut Vec<u8>) {
        while !bytes.len().is_multiple_of(8) {
            bytes.push(0);
        }
    }

    fn entry(key: &str, ty: &str, value: &[u8]) -> Vec<u8> {
        let mut body = format!("{key}\0").into_bytes();
        let key_end = body.len() as u64;
        pad(&mut body);
        body.extend_from_slice(value);
        body.push(0);
        body.extend_from_slice(ty.as_bytes());
        framed(&body, &[key_end])
    }

    pub(crate) fn gzip(text: &str) -> Vec<u8> {
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(text.as_bytes()).unwrap();
        encoder.finish().unwrap()
    }

    /// A bundle with these header entries, shaped like `flatpak
    /// build-bundle` writes them, and `tail` standing in for the rest.
    pub(crate) fn bundle(entries: &[(&str, &str, Vec<u8>)], tail: usize) -> Vec<u8> {
        let entries: Vec<_> = entries
            .iter()
            .map(|(key, ty, value)| entry(key, ty, value))
            .collect();
        framed_bundle(&entries, tail)
    }

    /// A bundle of already framed `{sv}` entries.
    fn framed_bundle(entries: &[Vec<u8>], tail: usize) -> Vec<u8> {
        let mut dict = Vec::new();
        let mut ends = Vec::new();
        for entry in entries {
            pad(&mut dict);
            dict.extend(entry);
            ends.push(dict.len() as u64);
        }
        let dict = framed(&dict, &ends);
        let mut body = dict.clone();
        body.extend(std::iter::repeat_n(b'x', tail));
        // The other members' offsets come first, the dictionary's last.
        let mut offsets = vec![body.len() as u64; 5];
        offsets.push(dict.len() as u64);
        framed(&body, &offsets)
    }

    pub(crate) fn app(reference: &str, version: Option<&str>) -> Vec<u8> {
        let mut entries = vec![
            ("flatpak", "u", 1u32.to_le_bytes().to_vec()),
            ("ref", "s", format!("{reference}\0").into_bytes()),
            (
                "metadata",
                "s",
                b"[Application]\nname=org.example.Synthetic\n\0".to_vec(),
            ),
        ];
        if let Some(version) = version {
            let xml = format!(
                "<component><releases>\n<release type=\"stable\" version=\"{version}\" \
                 timestamp=\"1\"/>\n<release version=\"0.1\"/></releases></component>"
            );
            entries.push(("appdata", "ay", gzip(&xml)));
        }
        entries.push(("deltas/part/0", "(yay)", vec![b'd'; 300]));
        bundle(&entries, 70_000)
    }

    fn parse(bytes: &[u8]) -> Result<Bundle, String> {
        read_from(&mut Cursor::new(bytes), bytes.len() as u64)
    }

    #[test]
    fn a_bundle_names_its_app_and_newest_release() {
        let bytes = app("app/org.example.Synthetic/x86_64/stable", Some("2.1.0"));
        assert_eq!(
            parse(&bytes),
            Ok(Bundle {
                id: "org.example.Synthetic".into(),
                arch: "x86_64".into(),
                branch: "stable".into(),
                version: Some("2.1.0".into()),
            })
        );
        let runtime = parse(&app("runtime/org.example.Platform/aarch64/24.08", None)).unwrap();
        assert_eq!(
            (
                runtime.id.as_str(),
                runtime.branch.as_str(),
                runtime.version
            ),
            ("org.example.Platform", "24.08", None)
        );
        // Small bundles frame with narrower offsets.
        let small = bundle(
            &[(
                "ref",
                "s",
                b"app/org.example.Small/x86_64/master\0".to_vec(),
            )],
            0,
        );
        assert_eq!(parse(&small).unwrap().id, "org.example.Small");
        // The bundle other crates' tests download.
        let fixture = parse(include_bytes!("backends/fixtures/synthetic.flatpak")).unwrap();
        assert_eq!(
            (fixture.id.as_str(), fixture.version.as_deref()),
            ("org.example.Synthetic", Some("1.0"))
        );
    }

    #[test]
    fn a_bundle_read_from_disk_matches() {
        let path = std::env::temp_dir().join(format!(
            "pkgdeck-flatpak-bundle-{}.flatpak",
            std::process::id()
        ));
        std::fs::write(&path, app("app/org.example.Disk/x86_64/stable", Some("1"))).unwrap();
        let bundle = read(&path);
        std::fs::remove_file(&path).unwrap();
        assert_eq!(bundle.unwrap().id, "org.example.Disk");
        assert!(read(Path::new("/nonexistent/pkgdeck.flatpak")).is_err());
    }

    #[test]
    fn offsets_widen_with_the_container() {
        for (len, size) in [
            (0, 0),
            (1, 1),
            (0xff, 1),
            (0x100, 2),
            (0xffff, 2),
            (0x1_0000, 4),
            (0xffff_ffff, 4),
            (0x1_0000_0000, 8),
            (u64::MAX, 8),
        ] {
            assert_eq!(offset_size(len), size, "{len:#x}");
        }
    }

    #[test]
    fn releases_are_read_only_from_a_release_version() {
        for (xml, expected) in [
            (r#"<releases><release version="3" /></releases>"#, Some("3")),
            (r"<release  version = '4.0~rc1'>", Some("4.0~rc1")),
            (r#"<release timestamp="1" version="5"/>"#, Some("5")),
            (r#"<release unversion="1" version="6"/>"#, Some("6")),
            (r#"<releases></releases>"#, None),
            (r#"<release date="2024"/>"#, None),
            (r#"<release version=""/>"#, None),
            (r#"<release version="1 2"/>"#, None),
            (r#"<release version="<x>"/>"#, None),
            (r#"<release version=7/>"#, None),
            (r#"<release version="8"#, None),
            ("<release", None),
        ] {
            assert_eq!(release_version(&gzip(xml)).as_deref(), expected, "{xml}");
        }
        assert_eq!(release_version(b"not gzip"), None);
    }

    #[test]
    fn broken_or_foreign_files_are_refused() {
        for reference in [
            "app/org.example.App/x86_64",
            "app/org.example.App/x86_64/stable/extra",
            "extension/org.example.App/x86_64/stable",
            "app//x86_64/stable",
            "app/org.example App/x86_64/stable",
        ] {
            assert!(parse(&app(reference, None)).is_err(), "{reference}");
        }
        // No ref, or a ref of the wrong type.
        assert!(parse(&bundle(&[("flatpak", "u", vec![1, 0, 0, 0])], 0)).is_err());
        assert!(parse(&bundle(&[("ref", "ay", b"app/a/b/c".to_vec())], 0)).is_err());
        let unterminated = bundle(&[("ref", "s", b"app/a/b/c".to_vec())], 0);
        assert!(parse(&unterminated).is_err());
        // A ref too long to be one is not read.
        let long = format!("app/{}/x86_64/stable\0", "a".repeat(2000));
        assert!(parse(&bundle(&[("ref", "s", long.into_bytes())], 0)).is_err());
        // A value with no zero byte before its type.
        let mut body = b"ref\0".to_vec();
        pad(&mut body);
        body.extend_from_slice(b"app/a/b/cs");
        let untyped = framed_bundle(&[framed(&body, &[4])], 0);
        assert!(parse(&untyped).is_err());
        for bytes in [&b""[..], b"x", b"\x00", b"flatpak\x00\x01"] {
            assert!(parse(bytes).is_err());
        }
    }

    /// Every truncation and single-byte change of a bundle either reads or
    /// fails; none panics or reads past the end.
    #[test]
    fn damaged_bundles_never_panic() {
        let bytes = app("app/org.example.Synthetic/x86_64/stable", Some("2"));
        let header = bytes.len() - 70_000;
        let mut cases = 0;
        for len in (0..bytes.len()).step_by(97).chain(header - 64..header + 64) {
            let _ = parse(&bytes[..len.min(bytes.len())]);
            cases += 1;
        }
        let interesting = (0..header).chain(bytes.len() - 16..bytes.len());
        for at in interesting {
            for value in [0x00, 0x01, 0x7f, 0xff] {
                let mut damaged = bytes.clone();
                damaged[at] = value;
                let _ = parse(&damaged);
                cases += 1;
            }
        }
        assert!(cases > 1000);
    }
}
