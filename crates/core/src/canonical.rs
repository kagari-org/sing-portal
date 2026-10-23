use std::collections::HashMap;

use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::error::Error;
use crate::model::{
    MAX_OUTBOUNDS_PER_SUBSCRIPTION, TAG_SEPARATOR, is_proxy_outbound, outbound_tag, outbound_type,
};

pub fn slugify(name: &str) -> String {
    let mut slug = String::new();
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
        } else if !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug: String = slug.trim_matches('-').chars().take(16).collect();
    let slug = slug.trim_end_matches('-').to_string();
    if slug.is_empty() { "sub".to_string() } else { slug }
}

pub fn unique_slug(base: &str, taken: &[String]) -> String {
    if !taken.iter().any(|slug| slug == base) {
        return base.to_string();
    }
    for index in 2.. {
        let candidate = format!("{base}-{index}");
        if !taken.iter().any(|slug| slug == &candidate) {
            return candidate;
        }
    }
    unreachable!("slug overflow")
}

pub fn normalize(outbounds: Vec<Value>, slug: &str) -> Result<Vec<Value>, Error> {
    if outbounds.len() > MAX_OUTBOUNDS_PER_SUBSCRIPTION {
        return Err(Error::TooManyOutbounds(
            outbounds.len(),
            MAX_OUTBOUNDS_PER_SUBSCRIPTION,
        ));
    }

    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut result = Vec::new();

    for mut outbound in outbounds {
        if !is_proxy_outbound(&outbound) || outbound_type(&outbound).is_none() {
            continue;
        }
        let Some(tag) = outbound_tag(&outbound).map(ToString::to_string) else {
            continue;
        };

        let base = format!("{slug}{TAG_SEPARATOR}{tag}");
        let count = seen.entry(base.clone()).or_insert(0);
        *count += 1;
        let unique = if *count == 1 {
            base
        } else {
            format!("{base}#{count}")
        };

        outbound["tag"] = Value::String(unique);
        result.push(outbound);
    }

    if result.is_empty() {
        return Err(Error::NoOutbounds);
    }

    sort_by_tag(&mut result);
    Ok(result)
}

pub fn sort_by_tag(outbounds: &mut [Value]) {
    outbounds.sort_by(|left, right| outbound_tag(left).cmp(&outbound_tag(right)));
}

pub fn revision(outbounds: &[Value]) -> String {
    if outbounds.is_empty() {
        return "none".to_string();
    }

    let mut sorted = outbounds.to_vec();
    sort_by_tag(&mut sorted);

    let mut hasher = Sha256::new();
    for outbound in &sorted {
        hasher.update(serde_json::to_vec(outbound).unwrap_or_default());
        hasher.update(b"\n");
    }
    format!("sha256:{}", hex::encode(hasher.finalize()))
}

pub fn selection_revision(node: &str, selection: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(node.as_bytes());
    hasher.update(b"\0");
    hasher.update(selection.as_bytes());
    format!("sha256:{}", hex::encode(hasher.finalize()))
}
