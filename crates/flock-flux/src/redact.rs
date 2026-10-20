use serde_json::{Map, Value};

pub const MASK: &str = "••••••";
pub const CHANGED: &str = "•••••• (changed)";
const SECRET_KIND: &str = "Secret";
const SECRET_FIELDS: [&str; 2] = ["data", "stringData"];
const MANAGED_FIELDS: &str = "managedFields";
const SECRET_MARKER: &str = "kind:Secret";
const MARKER_NOISE: [char; 2] = ['"', '\''];
const DOCUMENT_SEPARATOR: &str = "---";
const COMMENT: char = '#';
const SCHEME_SEPARATOR: &str = "://";
const AUTHORITY_END: [char; 3] = ['/', '?', '#'];
const WORD_END: [char; 5] = ['"', '\'', '<', '>', '`'];
const PLAIN_SCHEMES: [&str; 2] = ["http", "https"];
const JSON_START: [char; 2] = ['{', '['];
const SENSITIVE_KEYS: [&str; 8] = [
    "password",
    "passwd",
    "secret",
    "token",
    "apikey",
    "privatekey",
    "credential",
    "accesskey",
];
const KEY_NOISE: [char; 2] = ['-', '_'];

pub fn object(value: &mut Value) {
    redact(value, None);
}

pub fn object_against(value: &mut Value, previous: &Value) {
    redact(value, Some(previous));
}

pub fn values(value: &mut Value) {
    mask_sensitive(value);
    redact(value, None);
}

fn sensitive(key: &str) -> bool {
    let compact: String = key
        .chars()
        .filter(|c| !KEY_NOISE.contains(c))
        .collect::<String>()
        .to_lowercase();
    SENSITIVE_KEYS.iter().any(|word| compact.contains(word))
}

fn mask_sensitive(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for (key, child) in map.iter_mut() {
                let masked = sensitive(key) && !child.is_object() && !child.is_array();
                if masked && !child.is_null() {
                    *child = Value::from(MASK);
                } else {
                    mask_sensitive(child);
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(mask_sensitive),
        Value::Null => {}
        Value::Bool(_) => {}
        Value::Number(_) => {}
        Value::String(_) => {}
    }
}

pub fn manifest(text: &str) -> String {
    rewrite(text, &[])
}

pub fn manifests(older: &str, newer: &str) -> (String, String) {
    let previous: Vec<Value> = chunks(older)
        .into_iter()
        .filter_map(|chunk| serde_norway::from_str::<Value>(chunk).ok())
        .collect();
    (manifest(older), rewrite(newer, &previous))
}

pub fn credentials(text: &str) -> String {
    let mut clean = String::with_capacity(text.len());
    let mut word = String::new();
    for c in text.chars() {
        if c.is_whitespace() || WORD_END.contains(&c) {
            clean.push_str(&scrub_word(&word));
            word.clear();
            clean.push(c);
        } else {
            word.push(c);
        }
    }
    clean.push_str(&scrub_word(&word));
    clean
}

fn redact(value: &mut Value, previous: Option<&Value>) -> bool {
    match value {
        Value::Object(map) => redact_map(map, previous),
        Value::Array(items) => {
            let mut changed = false;
            for (ix, item) in items.iter_mut().enumerate() {
                let before = previous.and_then(|p| p.get(ix));
                changed |= redact(item, before);
            }
            changed
        }
        Value::String(text) => redact_string(text),
        Value::Null => false,
        Value::Bool(_) => false,
        Value::Number(_) => false,
    }
}

fn redact_map(map: &mut Map<String, Value>, previous: Option<&Value>) -> bool {
    let mut changed = false;
    if let Some(metadata) = map.get_mut("metadata").and_then(Value::as_object_mut) {
        let had = metadata.contains_key(MANAGED_FIELDS);
        metadata.retain(|key, _| key != MANAGED_FIELDS);
        changed |= had;
    }
    let secret = map.get("kind").and_then(Value::as_str) == Some(SECRET_KIND);
    for (key, child) in map.iter_mut() {
        let before = previous.and_then(|p| p.get(key));
        changed |= if secret && SECRET_FIELDS.contains(&key.as_str()) {
            mask_entries(child, before)
        } else {
            redact(child, before)
        };
    }
    changed
}

fn mask_entries(entries: &mut Value, previous: Option<&Value>) -> bool {
    if entries.is_null() {
        return false;
    }
    let Some(map) = entries.as_object_mut() else {
        *entries = Value::from(MASK);
        return true;
    };
    for (key, value) in map.iter_mut() {
        let rotated = previous
            .and_then(|p| p.get(key))
            .is_some_and(|before| before != value);
        *value = Value::from(if rotated { CHANGED } else { MASK });
    }
    true
}

fn redact_string(text: &mut String) -> bool {
    let scrubbed = credentials(text);
    let clean = if mentions_secret(&scrubbed) {
        embedded(&scrubbed)
    } else {
        scrubbed
    };
    let changed = clean != *text;
    *text = clean;
    changed
}

fn mentions_secret(text: &str) -> bool {
    let compact: String = text
        .chars()
        .filter(|c| !c.is_whitespace() && !MARKER_NOISE.contains(c))
        .collect();
    compact.contains(SECRET_MARKER)
}

fn embedded(text: &str) -> String {
    if !text.trim_start().starts_with(JSON_START) {
        return manifest(text);
    }
    let Ok(mut value) = serde_json::from_str::<Value>(text) else {
        return MASK.to_owned();
    };
    redact(&mut value, None);
    serde_json::to_string(&value).unwrap_or_else(|_| MASK.to_owned())
}

fn chunks(text: &str) -> Vec<&str> {
    let mut chunks = Vec::new();
    let mut start = 0;
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        if line.trim_end() == DOCUMENT_SEPARATOR && offset > start {
            chunks.extend(text.get(start..offset));
            start = offset;
        }
        offset += line.len();
    }
    chunks.extend(text.get(start..));
    chunks
}

fn identity(value: &Value) -> [Option<&str>; 4] {
    [
        value.get("apiVersion").and_then(Value::as_str),
        value.get("kind").and_then(Value::as_str),
        value.pointer("/metadata/namespace").and_then(Value::as_str),
        value.pointer("/metadata/name").and_then(Value::as_str),
    ]
}

fn header(chunk: &str) -> String {
    chunk
        .split_inclusive('\n')
        .take_while(|line| {
            let trimmed = line.trim();
            trimmed.is_empty() || trimmed == DOCUMENT_SEPARATOR || trimmed.starts_with(COMMENT)
        })
        .collect()
}

fn rewrite(text: &str, previous: &[Value]) -> String {
    chunks(text)
        .into_iter()
        .map(|chunk| rewrite_chunk(chunk, previous))
        .collect()
}

fn rewrite_chunk(chunk: &str, previous: &[Value]) -> String {
    let mut value = match serde_norway::from_str::<Value>(chunk) {
        Ok(value) => value,
        Err(_) if mentions_secret(chunk) => return format!("{}{MASK}\n", header(chunk)),
        Err(_) => return credentials(chunk),
    };
    let key = identity(&value);
    let before = previous.iter().find(|doc| identity(doc) == key);
    if !redact(&mut value, before) {
        return chunk.to_owned();
    }
    match serde_norway::to_string(&value) {
        Ok(body) => format!("{}{body}", header(chunk)),
        Err(_) => format!("{}{MASK}\n", header(chunk)),
    }
}

fn scrub_word(word: &str) -> String {
    match word.find(SCHEME_SEPARATOR) {
        Some(at) => scrub_url(word, at),
        None => scrub_scp(word),
    }
}

fn scrub_url(word: &str, at: usize) -> String {
    let (prefix, rest) = word.split_at(at + SCHEME_SEPARATOR.len());
    let scheme = prefix
        .get(..at)
        .unwrap_or_default()
        .rsplit(|c: char| !(c.is_ascii_alphanumeric() || "+.-".contains(c)))
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let end = rest.find(AUTHORITY_END).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(end);
    let host = match authority.rsplit_once('@') {
        Some((user, _)) if !user.contains(':') && !PLAIN_SCHEMES.contains(&scheme.as_str()) => {
            authority
        }
        Some((_, host)) => host,
        None => authority,
    };
    format!("{prefix}{host}{}", scrub_query(tail))
}

fn scrub_query(tail: &str) -> String {
    let Some((path, rest)) = tail.split_once('?') else {
        return tail.to_owned();
    };
    let (query, fragment) = match rest.split_once('#') {
        Some((query, fragment)) => (query, Some(fragment)),
        None => (rest, None),
    };
    let masked: Vec<String> = query
        .split('&')
        .map(|pair| match pair.split_once('=') {
            Some((key, value)) if !value.is_empty() => format!("{key}={MASK}"),
            Some(_) => pair.to_owned(),
            None => pair.to_owned(),
        })
        .collect();
    let fragment = fragment.map(|f| format!("#{f}")).unwrap_or_default();
    format!("{path}?{}{fragment}", masked.join("&"))
}

fn scrub_scp(word: &str) -> String {
    match word.rsplit_once('@') {
        Some((user, host))
            if user.contains(':') && !user.contains('/') && host.contains(['.', ':']) =>
        {
            host.to_owned()
        }
        Some(_) => word.to_owned(),
        None => word.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const PLANTED: &str = "planted-credential";

    fn last_applied(secret: &Value) -> Value {
        json!({
            "apiVersion": "v1",
            "kind": "Secret",
            "metadata": {
                "name": "db",
                "annotations": {
                    "kubectl.kubernetes.io/last-applied-configuration": secret.to_string(),
                },
                "managedFields": [{ "manager": "kubectl", "fieldsV1": { "f:data": { "f:password": {} } } }],
            },
            "data": { "password": "cGxhbnRlZA==" },
            "stringData": { "token": PLANTED },
        })
    }

    #[test]
    fn secrets_lose_their_values_everywhere_they_appear() {
        let applied = json!({
            "apiVersion": "v1",
            "kind": "Secret",
            "metadata": { "name": "db" },
            "stringData": { "password": PLANTED },
        });
        let mut secret = last_applied(&applied);
        object(&mut secret);
        let text = secret.to_string();
        assert!(!text.contains(PLANTED), "{text}");
        assert!(!text.contains("cGxhbnRlZA=="), "{text}");
        assert!(!text.contains(MANAGED_FIELDS), "{text}");
        assert_eq!(secret["data"]["password"], json!(MASK));
        assert!(text.contains("password"), "{text}");
    }

    #[test]
    fn malformed_secret_annotations_are_masked_whole() {
        let mut secret = json!({
            "kind": "ConfigMap",
            "metadata": { "annotations": { "a": format!("{{\"kind\":\"Secret\",\"data\":{{\"k\":\"{PLANTED}\"") } },
        });
        object(&mut secret);
        assert_eq!(secret["metadata"]["annotations"]["a"], json!(MASK));
    }

    #[test]
    fn secrets_embedded_in_other_kinds_are_redacted() {
        let manifest_text = format!(
            "apiVersion: v1\nkind: Secret\nmetadata:\n  name: x\nstringData:\n  key: {PLANTED}\n"
        );
        let mut list = json!({
            "kind": "ConfigMap",
            "data": { "manifest.yaml": manifest_text },
            "items": [{ "kind": "Secret", "data": { "k": PLANTED } }],
        });
        object(&mut list);
        let text = list.to_string();
        assert!(!text.contains(PLANTED), "{text}");
        assert!(text.contains("key"), "{text}");
    }

    #[test]
    fn manifests_keep_other_documents_verbatim_and_mark_rotations() {
        let older = format!(
            "---\n# Source: app/templates/cm.yaml\nkind: ConfigMap\nmetadata:\n  name: cm\ndata: {{ a: 1 }}\n---\n# Source: app/templates/secret.yaml\napiVersion: v1\nkind: Secret\nmetadata:\n  name: db\nstringData:\n  password: old-{PLANTED}\n  user: admin\n"
        );
        let newer = older.replace("old-", "new-");
        let (old_side, new_side) = manifests(&older, &newer);
        assert!(!old_side.contains(PLANTED), "{old_side}");
        assert!(!new_side.contains(PLANTED), "{new_side}");
        assert!(new_side.starts_with("---\n# Source: app/templates/cm.yaml\nkind: ConfigMap\nmetadata:\n  name: cm\ndata: { a: 1 }\n---\n# Source: app/templates/secret.yaml\n"), "{new_side}");
        assert!(
            new_side.contains(&format!("password: {CHANGED}")),
            "{new_side}"
        );
        assert!(new_side.contains(&format!("user: {MASK}")), "{new_side}");
        assert!(
            old_side.contains(&format!("password: {MASK}")),
            "{old_side}"
        );
    }

    #[test]
    fn malformed_secret_documents_are_masked() {
        let text = format!("---\nkind: Secret\ndata: [{PLANTED}\n");
        let clean = manifest(&text);
        assert!(!clean.contains(PLANTED), "{clean}");
    }

    #[test]
    fn credentials_leave_urls_without_userinfo_or_query_values() {
        assert_eq!(
            credentials("fetch https://x-access-token:ghs_abc@github.com/acme/repo.git failed"),
            "fetch https://github.com/acme/repo.git failed"
        );
        assert_eq!(
            credentials("https://ghp_token@github.com/acme/repo"),
            "https://github.com/acme/repo"
        );
        assert_eq!(
            credentials("'https://host/repo.git?token=abc&ref=main#frag'"),
            format!("'https://host/repo.git?token={MASK}&ref={MASK}#frag'")
        );
        assert_eq!(
            credentials("ssh://git@github.com/acme/repo.git"),
            "ssh://git@github.com/acme/repo.git"
        );
        assert_eq!(
            credentials("ssh://git:pw@github.com/acme/repo.git"),
            "ssh://github.com/acme/repo.git"
        );
        assert_eq!(
            credentials("x-access-token:abc@github.com:acme/repo.git"),
            "github.com:acme/repo.git"
        );
        assert_eq!(
            credentials("git@github.com:acme/repo.git and a@b.c"),
            "git@github.com:acme/repo.git and a@b.c"
        );
    }

    #[test]
    fn redacting_against_a_previous_object_marks_rotated_values_and_keeps_key_order() {
        let before = json!({
            "kind": "Secret",
            "metadata": { "managedFields": [], "name": "db", "namespace": "apps" },
            "data": { "password": "b2xk", "user": "YWRtaW4=" },
        });
        let mut after = json!({
            "kind": "Secret",
            "metadata": { "managedFields": [], "name": "db", "namespace": "apps" },
            "data": { "password": "bmV3", "user": "YWRtaW4=" },
        });
        object_against(&mut after, &before);
        assert_eq!(after["data"]["password"], json!(CHANGED));
        assert_eq!(after["data"]["user"], json!(MASK));
        let keys: Vec<&String> = after["metadata"]
            .as_object()
            .map(|metadata| metadata.keys().collect())
            .unwrap_or_default();
        assert_eq!(keys, ["name", "namespace"]);
    }

    #[test]
    fn plain_values_are_untouched() {
        let mut deployment = json!({
            "kind": "Deployment",
            "metadata": { "name": "web", "annotations": { "note": "Secret \"x\" not found" } },
            "spec": { "replicas": 2 },
        });
        let before = deployment.clone();
        object(&mut deployment);
        assert_eq!(deployment, before);
    }
}
