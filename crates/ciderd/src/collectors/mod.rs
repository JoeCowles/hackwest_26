//! Pure, bounded source adapters. Acquisition runs in independently supervised workers.
mod inventory;
mod mounts;
mod nfs;
mod smart;

pub use inventory::{parse_apfs, parse_inventory, parse_iokit, parse_snapshots};
pub use mounts::{parse_capacity, parse_mount_identity, parse_mounts};
pub use nfs::{parse_nfs, parse_nfs_status};
pub use smart::parse_smart;

use crate::model::{Attributes, Metric, Relationship, Resource};
use anyhow::{ensure, Context, Result};
use serde::{
    de::{DeserializeSeed, MapAccess, SeqAccess, Visitor},
    Deserialize, Deserializer,
};
use serde_json::Value;
use std::{collections::BTreeSet, fmt};
use uuid::Uuid;

pub const MAX_SOURCE_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_RECORDS: usize = 4096;

#[derive(Clone, Debug)]
pub struct Sample {
    pub resource_id: String,
    pub collector: String,
    pub metrics: Vec<Metric>,
    pub status: String,
    pub source_version: String,
    pub exit_code: Option<u8>,
}
#[derive(Clone, Debug, Default)]
pub struct Collected {
    pub resources: Vec<Resource>,
    pub relationships: Vec<Relationship>,
    pub samples: Vec<Sample>,
    pub complete: bool,
}
impl Collected {
    fn complete() -> Self {
        Self {
            complete: true,
            ..Self::default()
        }
    }
    fn sample(
        &mut self,
        resource: &str,
        collector: &str,
        metrics: Vec<Metric>,
        status: &str,
        version: &str,
    ) {
        self.samples.push(Sample {
            resource_id: resource.into(),
            collector: collector.into(),
            metrics,
            status: status.into(),
            source_version: version.into(),
            exit_code: None,
        });
    }
    fn resource(&mut self, resource: Resource) -> Result<()> {
        ensure!(self.resources.len() < MAX_RECORDS, "too many resources");
        ensure!(
            !self
                .resources
                .iter()
                .any(|v| v.resource_id == resource.resource_id),
            "duplicate source identity"
        );
        self.resources.push(resource);
        Ok(())
    }
}

pub fn scoped_id(node: &str, boot: &str, family: &str, identity: &str) -> String {
    let key = serde_json::to_vec(&(node, boot, family, identity)).expect("string tuple");
    Uuid::new_v5(&Uuid::NAMESPACE_OID, &key).to_string()
}
fn relation(from: &str, to: &str, kind: &str) -> Relationship {
    Relationship {
        relationship_id: scoped_id(from, to, "relationship", kind),
        revision: 1u64.into(),
        observed_at: crate::model::now(),
        from_resource_id: from.into(),
        to_resource_id: to.into(),
        relation: kind.into(),
        attributes: None,
    }
}
fn json(bytes: &[u8]) -> Result<Value> {
    ensure!(
        bytes.len() <= MAX_SOURCE_BYTES,
        "source output exceeds limit"
    );
    crate::model::parse_json(bytes)
}

// Deserialize plists structurally and reject duplicate dictionary keys before identity
// or counters are interpreted. Integers never pass through a floating-point value.
struct PlistJson(Value);
impl<'de> Deserialize<'de> for PlistJson {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        PlistSeed(0).deserialize(d)
    }
}
struct PlistSeed(usize);
impl<'de> DeserializeSeed<'de> for PlistSeed {
    type Value = PlistJson;
    fn deserialize<D: Deserializer<'de>>(self, d: D) -> std::result::Result<PlistJson, D::Error> {
        if self.0 >= 64 {
            return Err(serde::de::Error::custom("plist nesting exceeds limit"));
        }
        struct V(usize);
        impl<'de> Visitor<'de> for V {
            type Value = PlistJson;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("bounded plist value")
            }
            fn visit_map<A: MapAccess<'de>>(
                self,
                mut access: A,
            ) -> std::result::Result<PlistJson, A::Error> {
                let mut fields = serde_json::Map::new();
                while let Some(key) = access.next_key::<String>()? {
                    if fields.len() >= MAX_RECORDS || fields.contains_key(&key) {
                        return Err(serde::de::Error::custom(
                            "duplicate or excessive plist keys",
                        ));
                    }
                    fields.insert(key, access.next_value_seed(PlistSeed(self.0 + 1))?.0);
                }
                Ok(PlistJson(Value::Object(fields)))
            }
            fn visit_seq<A: SeqAccess<'de>>(
                self,
                mut access: A,
            ) -> std::result::Result<PlistJson, A::Error> {
                let mut values = Vec::new();
                while let Some(value) = access.next_element_seed(PlistSeed(self.0 + 1))? {
                    if values.len() >= MAX_RECORDS {
                        return Err(serde::de::Error::custom("excessive plist array"));
                    }
                    values.push(value.0);
                }
                Ok(PlistJson(Value::Array(values)))
            }
            fn visit_str<E: serde::de::Error>(
                self,
                value: &str,
            ) -> std::result::Result<PlistJson, E> {
                Ok(PlistJson(value.into()))
            }
            fn visit_string<E: serde::de::Error>(
                self,
                value: String,
            ) -> std::result::Result<PlistJson, E> {
                Ok(PlistJson(value.into()))
            }
            fn visit_bool<E: serde::de::Error>(
                self,
                value: bool,
            ) -> std::result::Result<PlistJson, E> {
                Ok(PlistJson(value.into()))
            }
            fn visit_u64<E: serde::de::Error>(
                self,
                value: u64,
            ) -> std::result::Result<PlistJson, E> {
                Ok(PlistJson(value.to_string().into()))
            }
            fn visit_i64<E: serde::de::Error>(
                self,
                value: i64,
            ) -> std::result::Result<PlistJson, E> {
                Ok(PlistJson(value.to_string().into()))
            }
            fn visit_f64<E: serde::de::Error>(
                self,
                value: f64,
            ) -> std::result::Result<PlistJson, E> {
                serde_json::Number::from_f64(value)
                    .map(|v| PlistJson(Value::Number(v)))
                    .ok_or_else(|| serde::de::Error::custom("nonfinite plist number"))
            }
        }
        d.deserialize_any(V(self.0))
    }
}
fn plist(bytes: &[u8]) -> Result<Value> {
    ensure!(
        bytes.len() <= MAX_SOURCE_BYTES,
        "source output exceeds limit"
    );
    Ok(plist::from_bytes::<PlistJson>(bytes)
        .context("invalid plist output")?
        .0)
}
fn text<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    let text = value
        .get(key)
        .and_then(Value::as_str)
        .with_context(|| format!("missing string field {key}"))?;
    ensure!(
        !text.is_empty() && text.len() <= 4096 && !text.contains('\0'),
        "invalid string field {key}"
    );
    Ok(text)
}
fn array<'a>(value: &'a Value, key: &str) -> Result<&'a Vec<Value>> {
    let array = value
        .get(key)
        .and_then(Value::as_array)
        .with_context(|| format!("missing array {key}"))?;
    ensure!(array.len() <= MAX_RECORDS, "excessive array length");
    Ok(array)
}
fn uint(value: &Value) -> Result<u128> {
    let string = match value {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => anyhow::bail!("expected unsigned integer"),
    };
    Ok(crate::model::Decimal::try_from(string)?.get())
}
fn integer_field(
    metrics: &mut Vec<Metric>,
    value: &Value,
    field: &str,
    name: &str,
    freshness: &str,
    epoch: Option<&str>,
) -> Result<()> {
    if let Some(value) = value.get(field) {
        metrics.push(Metric::integer(name, uint(value)?, freshness, epoch)?);
    }
    Ok(())
}
fn attribute(fields: &mut Attributes, source: &Value, input: &str, output: &str) {
    if let Some(value) = source.get(input) {
        fields.insert(output.into(), value.clone());
    }
}
fn validate_samples(collected: &Collected) -> Result<()> {
    ensure!(
        collected.samples.len() <= MAX_RECORDS && collected.relationships.len() <= 8192,
        "excessive source records"
    );
    for sample in &collected.samples {
        ensure!(sample.metrics.len() <= 4096, "too many source metrics");
        let mut keys = BTreeSet::new();
        for metric in &sample.metrics {
            metric.validate()?;
            ensure!(keys.insert(metric.key()), "duplicate metric key");
        }
    }
    Ok(())
}
