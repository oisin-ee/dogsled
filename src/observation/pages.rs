//! Frames of one reply. A reply is one JSON object of any size; a frame is one JSON line
//! of at most `limit` bytes. A reply that fits is one plain line. A larger reply is a head
//! line and lines of operations that build the rest. Nothing is dropped or cut to fit.

use crate::error::{Error, Result};
use serde::Deserialize;
use serde_json::Value;
use std::io::{BufRead, Read as _};

const CONTINUES: &str = "continues";
/// `],"continues":true}` and the newline.
const TAIL: usize = 21;
/// JSON escapes one control byte as six bytes.
const MAX_ESCAPE: usize = 6;

/// A value above this size is opened empty and filled by later operations, so one frame
/// always has room for several operations.
const fn atom(limit: usize) -> usize {
    limit / 4
}

fn invalid(detail: &str) -> Error {
    Error::Invalid(format!("observation unavailable: {detail}"))
}

fn child(at: &str, key: &str) -> String {
    format!("{at}/{}", key.replace('~', "~0").replace('/', "~1"))
}

fn opened(value: &Value) -> Option<&'static [u8]> {
    match value {
        Value::Object(_) => Some(b"{}"),
        Value::Array(_) => Some(b"[]"),
        Value::String(_) => Some(b"\"\""),
        _ => None,
    }
}

/// Operations that build values under JSON pointers. Each one is a complete JSON object.
struct Ops {
    atom: usize,
    done: Vec<Vec<u8>>,
}

impl Ops {
    fn operation(&mut self, name: &str, at: &str, field: &str, open: u8, body: &mut Vec<u8>) {
        if body.is_empty() {
            return;
        }
        let close = if open == b'{' { b'}' } else { b']' };
        let mut op = format!("{{\"{name}\":{{\"at\":{},\"{field}\":", Value::from(at)).into_bytes();
        op.push(open);
        op.append(body);
        op.push(close);
        op.extend_from_slice(b"}}");
        self.done.push(op);
    }

    /// One member or element: inline when it is small, otherwise opened and filled.
    fn place(
        &mut self,
        (name, at, field, open): (&str, &str, &str, u8),
        prefix: &[u8],
        value: &Value,
        target: &str,
        batch: &mut Vec<u8>,
    ) -> Result<()> {
        let bytes = serde_json::to_vec(value)?;
        let empty = opened(value).filter(|_| bytes.len() > self.atom);
        let item = empty.unwrap_or(&bytes);
        if !batch.is_empty() && batch.len() + prefix.len() + item.len() + 1 > self.atom {
            self.operation(name, at, field, open, batch);
        }
        if !batch.is_empty() {
            batch.push(b',');
        }
        batch.extend_from_slice(prefix);
        batch.extend_from_slice(item);
        if empty.is_some() {
            self.operation(name, at, field, open, batch);
            self.fill(target, value)?;
        }
        Ok(())
    }

    fn members<'a>(
        &mut self,
        at: &str,
        members: impl Iterator<Item = (&'a String, &'a Value)>,
    ) -> Result<()> {
        let mut batch = Vec::new();
        for (key, value) in members {
            let mut prefix = serde_json::to_vec(key)?;
            prefix.push(b':');
            let target = child(at, key);
            self.place(
                ("merge", at, "members", b'{'),
                &prefix,
                value,
                &target,
                &mut batch,
            )?;
        }
        self.operation("merge", at, "members", b'{', &mut batch);
        Ok(())
    }

    fn fill(&mut self, at: &str, value: &Value) -> Result<()> {
        match value {
            Value::Object(members) => self.members(at, members.iter()),
            Value::Array(elements) => {
                let mut batch = Vec::new();
                for (index, element) in elements.iter().enumerate() {
                    let target = format!("{at}/{index}");
                    self.place(
                        ("extend", at, "values", b'['),
                        &[],
                        element,
                        &target,
                        &mut batch,
                    )?;
                }
                self.operation("extend", at, "values", b'[', &mut batch);
                Ok(())
            }
            Value::String(text) => {
                let mut rest = text.as_str();
                while !rest.is_empty() {
                    let mut end = rest.len().min((self.atom / MAX_ESCAPE).max(4));
                    while !rest.is_char_boundary(end) {
                        end -= 1;
                    }
                    let (chunk, tail) = rest.split_at(end);
                    self.done.push(
                        format!(
                            "{{\"concat\":{{\"at\":{},\"text\":{}}}}}",
                            Value::from(at),
                            Value::from(chunk)
                        )
                        .into_bytes(),
                    );
                    rest = tail;
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }
}

/// One member of a reply, serialized once and shared by every reply that selects it.
#[derive(Debug)]
pub(crate) struct Member {
    /// `"key":value`, for a reply of one frame.
    plain: Vec<u8>,
    /// Operations that place the member under `at`, for a reply of several frames.
    ops: Vec<Vec<u8>>,
}

impl Member {
    /// `at` is the JSON pointer of the object that holds the member.
    pub fn new(at: &str, key: &str, value: &Value, limit: usize) -> Result<Self> {
        let mut plain = serde_json::to_vec(key)?;
        plain.push(b':');
        serde_json::to_writer(&mut plain, value)?;
        let mut ops = Ops {
            atom: atom(limit),
            done: Vec::new(),
        };
        ops.members(at, std::iter::once((&key.to_owned(), value)))?;
        Ok(Self {
            plain,
            ops: ops.done,
        })
    }
}

fn line(mut frame: Vec<u8>, limit: usize) -> Result<Vec<u8>> {
    frame.push(b'\n');
    if frame.len() > limit {
        return Err(Error::Invalid(
            "observation value cannot be divided into frames".into(),
        ));
    }
    Ok(frame)
}

fn pack<'a>(
    head: Vec<u8>,
    ops: impl Iterator<Item = &'a Vec<u8>>,
    limit: usize,
) -> Result<Vec<Vec<u8>>> {
    let mut frames = vec![head];
    let mut current: Vec<u8> = Vec::new();
    for op in ops {
        if !current.is_empty() && current.len() + op.len() + 1 + TAIL > limit {
            frames.push(std::mem::take(&mut current));
        }
        if current.is_empty() {
            current = format!("{{\"sequence\":{},\"ops\":[", frames.len()).into_bytes();
        } else {
            current.push(b',');
        }
        current.extend_from_slice(op);
    }
    if !current.is_empty() {
        frames.push(current);
    }
    let last = frames.len() - 1;
    frames
        .into_iter()
        .enumerate()
        .map(|(index, mut frame)| {
            match (index, index == last) {
                (0, true) => frame.push(b'}'),
                (0, false) => {
                    if frame.len() > 1 {
                        frame.push(b',');
                    }
                    frame.extend_from_slice(b"\"continues\":true}");
                }
                (_, true) => frame.extend_from_slice(b"]}"),
                (_, false) => frame.extend_from_slice(b"],\"continues\":true}"),
            }
            line(frame, limit)
        })
        .collect()
}

/// The frames of one reply. `scalars` is `"name":value,…` for the head, and is never empty. Each group is an
/// object member of the reply and the members that it holds; the group named `""` holds
/// members of the reply itself.
pub(crate) fn reply(
    scalars: &str,
    groups: &[(&str, Vec<&Member>)],
    limit: usize,
) -> Result<Vec<Vec<u8>>> {
    let join = |frame: &mut Vec<u8>, members: &[&Member]| {
        for (index, member) in members.iter().enumerate() {
            if index > 0 {
                frame.push(b',');
            }
            frame.extend_from_slice(&member.plain);
        }
    };
    let size: usize = groups
        .iter()
        .map(|(name, members)| {
            name.len()
                + 6
                + members
                    .iter()
                    .map(|member| member.plain.len() + 1)
                    .sum::<usize>()
        })
        .sum();
    let mut head = format!("{{{scalars}").into_bytes();
    if head.len() + size + 2 <= limit {
        for (name, members) in groups {
            if name.is_empty() {
                if !members.is_empty() {
                    head.push(b',');
                    join(&mut head, members);
                }
            } else {
                head.extend_from_slice(format!(",{}:{{", Value::from(*name)).as_bytes());
                join(&mut head, members);
                head.push(b'}');
            }
        }
        head.push(b'}');
        return Ok(vec![line(head, limit)?]);
    }
    for (name, _) in groups.iter().filter(|(name, _)| !name.is_empty()) {
        head.extend_from_slice(format!(",{}:{{}}", Value::from(*name)).as_bytes());
    }
    let ops = groups
        .iter()
        .flat_map(|(_, members)| members.iter().flat_map(|member| member.ops.iter()));
    pack(head, ops, limit)
}

/// The frames of one JSON object that has no members serialized in advance.
pub(crate) fn frames(value: &Value, limit: usize) -> Result<Vec<Vec<u8>>> {
    let plain = serde_json::to_vec(value)?;
    if plain.len() < limit {
        return Ok(vec![line(plain, limit)?]);
    }
    let Value::Object(members) = value else {
        return Err(Error::Invalid(
            "observation value cannot be divided into frames".into(),
        ));
    };
    let mut ops = Ops {
        atom: atom(limit),
        done: Vec::new(),
    };
    ops.members("", members.iter())?;
    pack(b"{".to_vec(), ops.done.iter(), limit)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Op {
    Merge {
        at: String,
        members: serde_json::Map<String, Value>,
    },
    Extend {
        at: String,
        values: Vec<Value>,
    },
    Concat {
        at: String,
        text: String,
    },
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Continuation {
    sequence: u64,
    ops: Vec<Op>,
    #[serde(default)]
    continues: bool,
}

fn apply(root: &mut Value, op: Op) -> Result<()> {
    let misplaced = || invalid("invalid continuation frame");
    match op {
        Op::Merge { at, members } => {
            let target = root
                .pointer_mut(&at)
                .and_then(Value::as_object_mut)
                .ok_or_else(misplaced)?;
            for (key, value) in members {
                if target.insert(key, value).is_some() {
                    return Err(misplaced());
                }
            }
        }
        Op::Extend { at, values } => root
            .pointer_mut(&at)
            .and_then(Value::as_array_mut)
            .ok_or_else(misplaced)?
            .extend(values),
        Op::Concat { at, text } => match root.pointer_mut(&at) {
            Some(Value::String(target)) => target.push_str(&text),
            _ => return Err(misplaced()),
        },
    }
    Ok(())
}

fn frame(reader: &mut impl BufRead, limit: usize) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader
        .take(limit as u64 + 1)
        .read_until(b'\n', &mut bytes)
        .map_err(|error| invalid(&error.to_string()))?;
    if bytes.is_empty() || bytes.last() != Some(&b'\n') || bytes.len() > limit {
        return Err(invalid("closed or oversized frame"));
    }
    Ok(bytes)
}

/// Read one complete reply. A reply that ends early is an error, never a shorter value.
pub(crate) fn read(reader: &mut impl BufRead, limit: usize) -> Result<Value> {
    let mut value: Value = serde_json::from_slice(&frame(reader, limit)?)?;
    let mut continues = value
        .as_object_mut()
        .and_then(|head| head.remove(CONTINUES))
        .is_some_and(|flag| flag == Value::Bool(true));
    let mut sequence = 0;
    while continues {
        let next: Continuation = serde_json::from_slice(&frame(reader, limit)?)
            .map_err(|_| invalid("invalid continuation frame"))?;
        sequence += 1;
        if next.sequence != sequence {
            return Err(invalid("continuation frame is out of order"));
        }
        for op in next.ops {
            apply(&mut value, op)?;
        }
        continues = next.continues;
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const LIMIT: usize = 4096;

    fn assembled(frames: &[Vec<u8>]) -> Value {
        for frame in frames {
            assert!(frame.len() <= LIMIT, "frame of {} bytes", frame.len());
            assert_eq!(
                frame.iter().position(|byte| *byte == b'\n'),
                Some(frame.len() - 1)
            );
        }
        let bytes = frames.concat();
        let mut reader = std::io::BufReader::new(bytes.as_slice());
        let value = read(&mut reader, LIMIT).unwrap();
        assert!(reader.fill_buf().unwrap().is_empty(), "unread frames");
        value
    }

    fn large() -> Value {
        let record = |index: usize| {
            json!({
                "id": format!("r-{index}"),
                "request": {"task": "t".repeat(index % 700), "path/with~odd": [index, null, true]},
                "note": "line\nbreak \u{1}\u{1f600} é".repeat(index % 9),
            })
        };
        json!({
            "version": 2,
            "epoch": "e",
            "empty": {"object": {}, "array": [], "text": ""},
            "queue": (0..1500).map(record).collect::<Vec<_>>(),
            "owners": (0..40).map(|index| (format!("w{index}:p/~{index}"), json!({"items": (0..30).map(record).collect::<Vec<_>>()}))).collect::<serde_json::Map<_, _>>(),
            "body": "\u{1}é😀x".repeat(4000),
            "nested": [[["deep".repeat(3000)], {"k": "v".repeat(5000)}], 7],
        })
    }

    #[test]
    fn a_reply_that_fits_is_one_plain_line() {
        let value = json!({"version": 2, "local": {"host": {"id": "a"}}, "gaps": []});
        let lines = frames(&value, LIMIT).unwrap();
        assert_eq!(lines.len(), 1);
        assert_eq!(serde_json::from_slice::<Value>(&lines[0]).unwrap(), value);
        assert_eq!(assembled(&lines), value);
    }

    #[test]
    fn a_large_value_arrives_exactly_in_bounded_frames() {
        let value = large();
        assert!(serde_json::to_vec(&value).unwrap().len() > 100 * LIMIT);
        let lines = frames(&value, LIMIT).unwrap();
        assert!(lines.len() > 100);
        assert_eq!(assembled(&lines), value);
    }

    #[test]
    fn selected_members_build_the_same_reply_in_one_frame_or_many() {
        let value = large();
        let members = |limit: usize| -> Vec<Member> {
            value
                .as_object()
                .unwrap()
                .iter()
                .filter(|(key, _)| !matches!(key.as_str(), "version" | "epoch"))
                .map(|(key, member)| Member::new("/views", key, member, limit).unwrap())
                .collect()
        };
        let scalars = "\"version\":2,\"epoch\":\"e\"";
        let expected =
            |views: Value| json!({"version": 2, "epoch": "e", "local": {}, "views": views});
        let mut all = value.clone();
        all.as_object_mut()
            .unwrap()
            .retain(|key, _| !matches!(key.as_str(), "version" | "epoch"));

        let paged = members(LIMIT);
        let groups = [("local", Vec::new()), ("views", paged.iter().collect())];
        let lines = reply(scalars, &groups, LIMIT).unwrap();
        assert!(lines.len() > 100);
        assert_eq!(assembled(&lines), expected(all.clone()));

        let small = [("local", Vec::new()), ("views", vec![&paged[1]])];
        let lines = reply(scalars, &small, LIMIT).unwrap();
        assert_eq!(lines.len(), 1);
        assert_eq!(
            assembled(&lines),
            expected(json!({"empty": value["empty"]}))
        );

        let roomy = 64 * 1024 * 1024;
        let whole = members(roomy);
        let groups = [("local", Vec::new()), ("views", whole.iter().collect())];
        let lines = reply(scalars, &groups, roomy).unwrap();
        assert_eq!(lines.len(), 1);
        assert_eq!(
            serde_json::from_slice::<Value>(&lines[0]).unwrap(),
            expected(all)
        );
    }

    #[test]
    fn a_reply_that_ends_early_or_out_of_order_is_an_error() {
        let lines = frames(&large(), LIMIT).unwrap();
        let read_all = |lines: &[Vec<u8>]| {
            let bytes = lines.concat();
            read(&mut std::io::BufReader::new(bytes.as_slice()), LIMIT)
        };
        let error = read_all(&lines[..lines.len() - 1]).unwrap_err().to_string();
        assert!(error.contains("closed or oversized frame"), "{error}");
        let mut swapped = lines.clone();
        swapped.swap(1, 2);
        let error = read_all(&swapped).unwrap_err().to_string();
        assert!(error.contains("out of order"), "{error}");
        let mut cut = lines[..2].to_vec();
        let half = cut[1].len() / 2;
        cut[1].truncate(half);
        assert!(read_all(&cut).is_err());
    }

    #[test]
    fn a_frame_over_the_limit_is_refused() {
        let mut bytes = serde_json::to_vec(&json!({"text": "x".repeat(LIMIT)})).unwrap();
        bytes.push(b'\n');
        let error = read(&mut std::io::BufReader::new(bytes.as_slice()), LIMIT)
            .unwrap_err()
            .to_string();
        assert!(error.contains("closed or oversized frame"), "{error}");
    }

    #[test]
    fn an_operation_cannot_replace_or_misplace_a_value() {
        for ops in [
            json!([{"merge": {"at": "", "members": {"version": 3}}}]),
            json!([{"extend": {"at": "/version", "values": [1]}}]),
            json!([{"concat": {"at": "/missing", "text": "x"}}]),
        ] {
            let bytes = format!(
                "{}\n{}\n",
                json!({"version": 2, "continues": true}),
                json!({"sequence": 1, "ops": ops})
            );
            let error = read(&mut std::io::BufReader::new(bytes.as_bytes()), LIMIT)
                .unwrap_err()
                .to_string();
            assert!(error.contains("invalid continuation frame"), "{error}");
        }
    }
}
