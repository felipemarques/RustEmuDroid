use std::path::PathBuf;

use roxmltree::{Document, Node};

use super::{Component, ImageChoice, InstallPlan, fetch_metadata};
use crate::{Result, locale::Locale};

const REPOSITORY: &str = "https://dl.google.com/android/repository/";
const AOSP: &str = "https://dl.google.com/android/repository/sys-img/android/";
const PLAY: &str = "https://dl.google.com/android/repository/sys-img/google_apis_playstore/";

pub(super) fn resolve(choice: ImageChoice, locale: Locale) -> Result<InstallPlan> {
    let host = std::env::consts::OS;
    if std::env::consts::ARCH != "x86_64" || !matches!(host, "windows" | "linux") {
        return Err(locale
            .pick(
                "A instalação automática requer Windows ou Linux x86-64.",
                "Automatic installation requires x86-64 Windows or Linux.",
                "La instalación automática requiere Windows o Linux x86-64.",
            )
            .into());
    }
    let image = choice.package();
    let image_base = match choice {
        ImageChoice::Original => AOSP,
        ImageChoice::GooglePlay => PLAY,
    };
    let tools_xml = fetch_metadata(&format!("{REPOSITORY}repository2-3.xml"))?;
    let image_xml = fetch_metadata(&format!("{image_base}sys-img2-3.xml"))?;
    let tools = Document::parse(&tools_xml)?;
    let images = Document::parse(&image_xml)?;
    let mut licenses = Vec::new();
    let components = [
        package(&tools, REPOSITORY, "emulator", host, locale, &mut licenses)?,
        package(
            &tools,
            REPOSITORY,
            "platform-tools",
            host,
            locale,
            &mut licenses,
        )?,
        package(&images, image_base, image, host, locale, &mut licenses)?,
    ];
    let total_bytes = components.iter().try_fold(0u64, |total, item| {
        total
            .checked_add(item.bytes)
            .ok_or_else(|| invalid(locale, image, "size"))
    })?;
    Ok(InstallPlan {
        image: image.to_owned(),
        total_bytes,
        licenses,
        components: Vec::from(components),
    })
}

fn child<'a, 'i>(node: Node<'a, 'i>, name: &str) -> Option<Node<'a, 'i>> {
    node.children()
        .find(|n| n.is_element() && n.tag_name().name() == name)
}

fn field<'a, 'i>(node: Node<'a, 'i>, name: &str) -> Option<&'a str> {
    child(node, name)?.text().map(str::trim)
}

fn invalid(locale: Locale, id: &str, field: &str) -> String {
    let reason = locale.pick(
        "Campo inválido no catálogo Android",
        "Invalid field in the Android catalog",
        "Campo no válido en el catálogo de Android",
    );
    format!("{reason} ({id}, {field})")
}

fn stable_revision(package: Node<'_, '_>) -> Option<[u32; 3]> {
    if package.attribute("obsolete") == Some("true")
        || child(package, "obsolete").is_some()
        || child(package, "channelRef").is_some_and(|n| n.attribute("ref") != Some("channel-0"))
    {
        return None;
    }
    let revision = child(package, "revision")?;
    if field(revision, "preview").unwrap_or("0") != "0" {
        return None;
    }
    Some([
        field(revision, "major")?.parse().ok()?,
        field(revision, "minor").unwrap_or("0").parse().ok()?,
        field(revision, "micro").unwrap_or("0").parse().ok()?,
    ])
}

// A system image can be identical on both hosts. The emulator and adb are host executables.
fn host_match(archive: Node<'_, '_>, host: &str, image: bool) -> Option<u8> {
    let host_os = field(archive, "host-os");
    if !(host_os == Some(host) || (image && host_os.is_none())) {
        return None;
    }
    let arch = field(archive, "host-arch");
    if !matches!(arch, None | Some("x64" | "x86_64" | "amd64"))
        || !matches!(field(archive, "host-bits"), None | Some("64"))
    {
        return None;
    }
    Some(2 * u8::from(host_os.is_some()) + u8::from(arch.is_some()))
}

fn package(
    doc: &Document<'_>,
    base: &str,
    id: &str,
    host: &str,
    locale: Locale,
    licenses: &mut Vec<String>,
) -> Result<Component> {
    let root = doc.root_element();
    let image = id.starts_with("system-images;");
    let available = root
        .children()
        .filter(|n| n.has_tag_name("remotePackage") && n.attribute("path") == Some(id));
    let selected = available
        .filter_map(|item| {
            let revision = stable_revision(item)?;
            let archives = child(item, "archives")?;
            let (score, complete) = archives
                .children()
                .filter(|n| n.has_tag_name("archive"))
                .filter_map(|a| Some((host_match(a, host, image)?, child(a, "complete")?)))
                .max_by_key(|(score, _)| *score)?;
            Some(((revision, score), item, complete))
        })
        .max_by_key(|(key, _, _)| *key);
    let (_, item, complete) = selected.ok_or_else(|| {
        let reason = locale.pick(
            "O catálogo oficial não contém um arquivo completo e estável compatível",
            "The official catalog has no compatible stable full archive",
            "El catálogo oficial no contiene un archivo completo y estable compatible",
        );
        format!("{reason} ({id}, {host} x86-64)")
    })?;
    let name = field(complete, "url")
        .filter(|s| {
            s.ends_with(".zip")
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        })
        .ok_or_else(|| invalid(locale, id, "url"))?;
    let bytes = field(complete, "size")
        .and_then(|s| s.parse::<u64>().ok())
        .filter(|n| *n > 0)
        .ok_or_else(|| invalid(locale, id, "size"))?;
    let checksum = complete
        .children()
        .filter(|n| n.has_tag_name("checksum"))
        .filter_map(|n| {
            let text = n.text()?.trim();
            let kind = n.attribute("type");
            let supported = match text.len() {
                40 => matches!(kind, None | Some("sha1" | "sha-1")),
                64 => matches!(kind, None | Some("sha256" | "sha-256")),
                _ => false,
            };
            (supported && text.bytes().all(|b| b.is_ascii_hexdigit())).then_some(text)
        })
        .max_by_key(|s| s.len())
        .ok_or_else(|| invalid(locale, id, "checksum"))?;
    let mut refs = item
        .children()
        .filter(|n| n.has_tag_name("uses-license"))
        .peekable();
    if refs.peek().is_none() {
        return Err(invalid(locale, id, "uses-license").into());
    }
    for reference in refs {
        let key = reference
            .attribute("ref")
            .ok_or_else(|| invalid(locale, id, "uses-license/ref"))?;
        let terms = root
            .children()
            .find(|n| n.has_tag_name("license") && n.attribute("id") == Some(key))
            .and_then(|n| n.text())
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| invalid(locale, id, key))?;
        if !licenses.iter().any(|existing| existing == terms) {
            licenses.push(terms.to_owned());
        }
    }
    Ok(Component {
        package: id.to_owned(),
        url: format!("{base}{name}"),
        bytes,
        checksum: checksum.to_ascii_lowercase(),
        relative: id.split(';').collect::<PathBuf>(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const HASH: &str = "0123456789abcdef0123456789abcdef01234567";

    fn archive(os: &str, arch: &str, file: &str) -> String {
        let os = if os.is_empty() {
            String::new()
        } else {
            format!("<host-os>{os}</host-os>")
        };
        let arch = if arch.is_empty() {
            String::new()
        } else {
            format!("<host-arch>{arch}</host-arch>")
        };
        format!(
            r#"<archive>{os}{arch}<complete><size>321</size>
             <checksum type="sha1">{HASH}</checksum><url>{file}</url></complete></archive>"#
        )
    }

    fn entry(id: &str, revision: &str, attributes: &str, archives: &str) -> String {
        format!(
            r#"<remotePackage path="{id}"><revision>{revision}</revision>{attributes}
             <uses-license ref="sdk"/><archives>{archives}</archives></remotePackage>"#
        )
    }

    fn index(packages: &str) -> String {
        format!(r#"<repository><license id="sdk">Agreement&#10;</license>{packages}</repository>"#)
    }

    #[test]
    fn selects_stable_revision_and_native_archives_for_each_host() {
        let native = [
            archive("windows", "x64", "win.zip"),
            archive("linux", "x64", "linux.zip"),
            archive("windows", "aarch64", "wrong-arch.zip"),
            archive("windows", "", "unspecified-arch.zip"),
            archive("", "", "unspecified-os.zip"),
        ]
        .concat();
        let older = archive("windows", "x64", "older.zip");
        let unstable = archive("windows", "x64", "unstable.zip");
        let packages = [
            entry(
                "emulator",
                "<major>10</major>",
                r#"<channelRef ref="channel-0"/>"#,
                &native,
            ),
            entry("emulator", "<major>9</major>", "", &older),
            entry(
                "emulator",
                "<major>11</major>",
                r#"<channelRef ref="channel-3"/>"#,
                &unstable,
            ),
            entry(
                "emulator",
                "<major>12</major><preview>1</preview>",
                "",
                &unstable,
            ),
        ]
        .concat();
        let xml = index(&packages);
        let doc = Document::parse(&xml).unwrap();
        let mut licenses = Vec::new();
        for (host, file) in [("windows", "win.zip"), ("linux", "linux.zip")] {
            let item = package(
                &doc,
                REPOSITORY,
                "emulator",
                host,
                Locale::EnUs,
                &mut licenses,
            )
            .unwrap();
            assert_eq!(item.url, format!("{REPOSITORY}{file}"));
            assert_eq!(item.bytes, 321);
            assert_eq!(item.checksum, HASH);
        }
        assert_eq!(licenses, ["Agreement\n"]);
    }

    #[test]
    fn resolves_exact_host_independent_image_and_reports_missing_id() {
        let id = "system-images;android-37.0;google_apis_playstore;x86_64";
        let xml = index(&entry(
            id,
            "<major>8</major>",
            "",
            &archive("", "", "image.zip"),
        ));
        let doc = Document::parse(&xml).unwrap();
        let mut licenses = vec!["Agreement\n".to_owned()];
        let item = package(&doc, PLAY, id, "linux", Locale::EnUs, &mut licenses).unwrap();
        assert_eq!(
            item.relative,
            PathBuf::from("system-images/android-37.0/google_apis_playstore/x86_64")
        );
        assert_eq!(licenses, ["Agreement\n"]);
        let missing = "system-images;android-37;google_apis_playstore;x86_64";
        let err = package(&doc, PLAY, missing, "linux", Locale::EnUs, &mut licenses)
            .err()
            .unwrap();
        assert!(err.to_string().contains(missing));
    }

    #[test]
    fn requires_complete_archive_known_license_and_valid_integrity_fields() {
        let good = index(&entry(
            "platform-tools",
            "<major>1</major>",
            "",
            &archive("windows", "", "adb.zip"),
        ));
        for bad in [
            good.replace("ref=\"sdk\"", "ref=\"missing\""),
            good.replace("<complete>", "<patch>")
                .replace("</complete>", "</patch>"),
            good.replace(HASH, "bad"),
            good.replace("<size>321", "<size>0"),
            good.replace("adb.zip", "../adb.zip"),
        ] {
            let doc = Document::parse(&bad).unwrap();
            assert!(
                package(
                    &doc,
                    REPOSITORY,
                    "platform-tools",
                    "windows",
                    Locale::EnUs,
                    &mut Vec::new()
                )
                .is_err()
            );
        }
    }
}
