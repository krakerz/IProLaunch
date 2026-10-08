//! Cover art for Sunshine app entries: SteamGridDB first, then Steam's own
//! store artwork. Sunshine only accepts a local PNG as `image-path`, so covers
//! are downloaded, converted to PNG if needed, and cached per profile slug.

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use serde_json::Value;

use crate::config::{Profile, project_dirs};

const SGDB_API: &str = "https://www.steamgriddb.com/api/v2";
/// Portrait grid sizes, closest to Moonlight's 3:4 box art first.
const SGDB_DIMENSIONS: &str = "600x900,660x930,342x482";
const STEAM_ASSETS: &str = "https://shared.steamstatic.com/store_item_assets/";

fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// Lowercase alphanumerics only, so "Rebirth: Pub" == "rebirth pub".
fn normalize(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// Names to look the game up by: its title (the official name used for
/// GAMEID matching) first, then its display name without the `#N`.
pub fn names_for(profile: &Profile) -> Vec<String> {
    let base = profile
        .name
        .rsplit_once('#')
        .map_or(profile.name.as_str(), |(base, _)| base)
        .trim()
        .to_string();
    let mut names: Vec<String> = Vec::new();
    for name in [profile.title.clone().unwrap_or_default(), base] {
        let name = name.trim().to_string();
        if !name.is_empty() && !names.iter().any(|n| normalize(n) == normalize(&name)) {
            names.push(name);
        }
    }
    names
}

/// First `(id, name)` whose name is exactly `wanted` once normalized.
fn exact_match(results: &[(u64, String)], wanted: &str) -> Option<u64> {
    let wanted = normalize(wanted);
    results
        .iter()
        .find(|(_, name)| normalize(name) == wanted)
        .map(|(id, _)| *id)
}

fn get_json(url: &str, bearer: Option<&str>) -> Result<Value> {
    let mut req = ureq::get(url);
    if let Some(key) = bearer {
        req = req.header("Authorization", &format!("Bearer {key}"));
    }
    req.call()
        .map_err(|err| match err {
            ureq::Error::StatusCode(401) => anyhow::anyhow!("SteamGridDB rejected the API key"),
            other => anyhow::Error::from(other),
        })?
        .body_mut()
        .read_json()
        .with_context(|| format!("reading {url}"))
}

fn sgdb_data(json: &Value) -> Vec<Value> {
    json.get("data")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn sgdb_search_results(json: &Value) -> Vec<(u64, String)> {
    sgdb_data(json)
        .iter()
        .filter_map(|g| Some((g.get("id")?.as_u64()?, g.get("name")?.as_str()?.to_string())))
        .collect()
}

fn sgdb_cover_url(key: &str, name: &str) -> Result<Option<String>> {
    let search = get_json(
        &format!("{SGDB_API}/search/autocomplete/{}", encode(name)),
        Some(key),
    )?;
    let Some(game_id) = exact_match(&sgdb_search_results(&search), name) else {
        return Ok(None);
    };
    let grids = get_json(
        &format!(
            "{SGDB_API}/grids/game/{game_id}?dimensions={}&mimes=image/png&types=static&nsfw=false",
            encode(SGDB_DIMENSIONS)
        ),
        Some(key),
    )?;
    Ok(sgdb_data(&grids)
        .first()
        .and_then(|g| g.get("url"))
        .and_then(Value::as_str)
        .map(str::to_string))
}

fn steam_search_results(json: &Value) -> Vec<(u64, String)> {
    json.get("items")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|i| i.get("type").and_then(Value::as_str) == Some("app"))
        .filter_map(|i| Some((i.get("id")?.as_u64()?, i.get("name")?.as_str()?.to_string())))
        .collect()
}

fn steam_appid_for(name: &str) -> Result<Option<u64>> {
    let json = get_json(
        &format!(
            "https://store.steampowered.com/api/storesearch/?term={}&l=english&cc=US",
            encode(name)
        ),
        None,
    )?;
    Ok(exact_match(&steam_search_results(&json), name))
}

/// Portrait library capsule URL from a `GetItems` store item's `assets`.
fn steam_capsule_from_assets(assets: &Value) -> Option<String> {
    let format = assets.get("asset_url_format")?.as_str()?;
    let file = assets
        .get("library_capsule_2x")
        .or_else(|| assets.get("library_capsule"))?
        .as_str()?;
    Some(format!(
        "{STEAM_ASSETS}{}",
        format.replace("${FILENAME}", file)
    ))
}

/// Steam's own portrait cover for `appid`. Newer games keep their artwork
/// under hashed paths, so the URL has to come from the store API.
fn steam_cover_url(appid: u64) -> Result<Option<String>> {
    let input = format!(
        r#"{{"ids":[{{"appid":{appid}}}],"context":{{"language":"english","country_code":"US"}},"data_request":{{"include_assets":true}}}}"#
    );
    let json = get_json(
        &format!(
            "https://api.steampowered.com/IStoreBrowseService/GetItems/v1?input_json={}",
            encode(&input)
        ),
        None,
    )?;
    Ok(json
        .pointer("/response/store_items/0/assets")
        .and_then(steam_capsule_from_assets))
}

pub fn covers_dir() -> Result<PathBuf> {
    Ok(project_dirs()?.data_dir().join("covers"))
}

fn cover_path(slug: &str) -> Result<PathBuf> {
    Ok(covers_dir()?.join(format!("{slug}.png")))
}

pub fn cached_cover(slug: &str) -> Option<PathBuf> {
    cover_path(slug).ok().filter(|p| p.is_file())
}

/// PNG bytes as-is; anything else (Steam serves JPEG) is re-encoded as PNG.
fn to_png(bytes: Vec<u8>) -> Result<Vec<u8>> {
    if bytes.starts_with(b"\x89PNG") {
        return Ok(bytes);
    }
    let img = image::load_from_memory(&bytes).context("decoding the cover image")?;
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png)
        .context("converting the cover to PNG")?;
    Ok(out.into_inner())
}

fn download(url: &str) -> Result<Vec<u8>> {
    ureq::get(url)
        .call()
        .with_context(|| format!("downloading {url}"))?
        .body_mut()
        .with_config()
        .limit(20 * 1024 * 1024)
        .read_to_vec()
        .context("reading the cover")
}

/// Finds a cover URL: the profile's Steam app ID override, then SteamGridDB
/// (with a key), then the Steam store — by exact name, title before name.
fn find_cover_url(sgdb_key: Option<&str>, profile: &Profile) -> Result<String> {
    if let Some(appid) = profile.defaults.steam_appid {
        return steam_cover_url(appid.into())?
            .with_context(|| format!("Steam has no library cover for app {appid}"));
    }
    let names = names_for(profile);
    if let Some(key) = sgdb_key {
        for name in &names {
            if let Some(url) = sgdb_cover_url(key, name)? {
                return Ok(url);
            }
        }
    }
    for name in &names {
        if let Some(appid) = steam_appid_for(name)?
            && let Some(url) = steam_cover_url(appid)?
        {
            return Ok(url);
        }
    }
    bail!(
        "no cover found (tried {}) — set the game's Steam app ID in its profile",
        names
            .iter()
            .map(|n| format!("\"{n}\""))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

/// Downloads `profile`'s cover (unless cached and `refetch` is false) and
/// returns its local PNG path.
pub fn fetch_cover(
    sgdb_key: Option<&str>,
    slug: &str,
    profile: &Profile,
    refetch: bool,
) -> Result<PathBuf> {
    if !refetch && let Some(path) = cached_cover(slug) {
        return Ok(path);
    }
    let url = find_cover_url(sgdb_key.filter(|k| !k.is_empty()), profile)?;
    let png = to_png(download(&url)?)?;
    let path = cover_path(slug)?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    fs::write(&path, png).with_context(|| format!("writing {}", path.display()))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(name: &str, title: Option<&str>) -> Profile {
        Profile {
            name: name.into(),
            target_path: "/tmp/game.exe".into(),
            title: title.map(str::to_string),
            last_launched: None,
            defaults: Default::default(),
            logging: Default::default(),
            env: Default::default(),
            winedlloverride: Default::default(),
            args: Vec::new(),
        }
    }

    #[test]
    fn names_try_title_first_then_name_without_counter() {
        assert_eq!(
            names_for(&profile("Rebirth Pub#1", Some("Rebirth Pub: Remastered"))),
            vec!["Rebirth Pub: Remastered", "Rebirth Pub"]
        );
        assert_eq!(names_for(&profile("Hades#2", Some("HADES"))), vec!["HADES"]);
        assert_eq!(names_for(&profile("Hades", Some("  "))), vec!["Hades"]);
    }

    #[test]
    fn exact_match_ignores_case_and_punctuation_but_not_extra_words() {
        let results = vec![(1, "Rebirth".to_string()), (2, "Rebirth: Pub".to_string())];
        assert_eq!(exact_match(&results, "rebirth pub"), Some(2));
        assert_eq!(exact_match(&results[..1], "Rebirth Pub"), None);
    }

    #[test]
    fn parses_steamgriddb_and_steam_search_results() {
        let sgdb: Value =
            serde_json::from_str(r#"{"success":true,"data":[{"id":5,"name":"Hades"}]}"#).unwrap();
        assert_eq!(sgdb_search_results(&sgdb), vec![(5, "Hades".to_string())]);

        let steam: Value = serde_json::from_str(
            r#"{"total":2,"items":[{"type":"bundle","name":"Rebirth Pub","id":9},
                {"type":"app","name":"Rebirth Pub","id":3236900}]}"#,
        )
        .unwrap();
        assert_eq!(
            steam_search_results(&steam),
            vec![(3236900, "Rebirth Pub".to_string())]
        );
    }

    #[test]
    fn builds_capsule_urls_for_hashed_and_plain_assets() {
        let hashed: Value = serde_json::from_str(
            r#"{"asset_url_format":"steam/apps/3236900/${FILENAME}?t=1786804921",
                "library_capsule":"cdbc/library_capsule.jpg",
                "library_capsule_2x":"cdbc/library_capsule_2x.jpg"}"#,
        )
        .unwrap();
        assert_eq!(
            steam_capsule_from_assets(&hashed).unwrap(),
            "https://shared.steamstatic.com/store_item_assets/steam/apps/3236900/cdbc/library_capsule_2x.jpg?t=1786804921"
        );
        let plain: Value = serde_json::from_str(
            r#"{"asset_url_format":"steam/apps/620/${FILENAME}?t=1","library_capsule":"library_600x900.jpg"}"#,
        )
        .unwrap();
        assert!(
            steam_capsule_from_assets(&plain)
                .unwrap()
                .ends_with("steam/apps/620/library_600x900.jpg?t=1")
        );
        assert!(steam_capsule_from_assets(&serde_json::json!({})).is_none());
    }

    #[test]
    fn converts_jpeg_to_png() {
        let mut jpeg = std::io::Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(4, 6)
            .write_to(&mut jpeg, image::ImageFormat::Jpeg)
            .unwrap();
        let png = to_png(jpeg.into_inner()).unwrap();
        assert!(png.starts_with(b"\x89PNG"));
    }

    #[test]
    fn encodes_names_for_urls() {
        assert_eq!(encode("Baldur's Gate 3"), "Baldur%27s%20Gate%203");
    }
}
