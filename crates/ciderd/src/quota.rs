//! Read-only rquota v1 over UDP. Called only in deadline-supervised subprocesses.
//! Wire definitions: quota-tools rquota.x and macOS SDK rpcsvc/rquota.x.
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    net::{ToSocketAddrs, UdpSocket},
    time::{Duration, Instant},
};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct QuotaTarget {
    pub server: String,
    /// Server-local export path understood by rquotad, not an NFSv4 pseudo path.
    pub export_path: String,
    pub uid: u32,
    #[serde(default)]
    pub port: Option<u16>,
    #[serde(default)]
    pub display_label: Option<String>,
}
impl QuotaTarget {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            !self.server.is_empty()
                && self.server.len() <= 253
                && !self.server.chars().any(|c| c.is_control()
                    || c.is_whitespace()
                    || matches!(c, '/' | '@' | '[' | ']')),
            "invalid quota server"
        );
        ensure!(
            self.export_path.starts_with('/')
                && self.export_path.len() <= 1024
                && !self.export_path.chars().any(char::is_control),
            "invalid quota export path"
        );
        ensure!(
            self.uid <= i32::MAX as u32,
            "rquota v1 UID must fit signed 32 bits"
        );
        ensure!(self.port != Some(0), "quota port cannot be zero");
        if let Some(label) = &self.display_label {
            ensure!(
                !label.is_empty() && label.len() <= 128 && !label.chars().any(char::is_control),
                "invalid quota display label"
            );
        }
        Ok(())
    }
    pub fn resource_id(&self, node: &str) -> String {
        crate::collectors::scoped_id(
            node,
            "",
            "nfs-user-quota",
            &serde_json::to_string(&(&self.server, &self.export_path, self.uid))
                .expect("string tuple"),
        )
    }
    pub fn resource(&self, node: &str) -> crate::model::Resource {
        crate::model::Resource::new(
            self.resource_id(node),
            "nfs_user_quota",
            crate::model::attrs(json!({
                "server":self.server,"export_path":self.export_path,"uid":self.uid.to_string(),"display_label":self.display_label,
                "nfs_source":format!("{}:{}",self.server,self.export_path),"rquotad_port":self.port,
                "protocol":"rquota-v1-udp","source":"nfs.rquota","quota_scope":"server_filesystem_user",
                "linkage_basis":"explicit_configuration","limits_zero_semantics":"unlimited_only_on_available_response"
            })),
        )
    }
}
struct Xdr<'a> {
    bytes: &'a [u8],
    offset: usize,
}
impl<'a> Xdr<'a> {
    fn word(&mut self) -> Result<u32> {
        let end = self.offset.checked_add(4).context("XDR overflow")?;
        let b = self.bytes.get(self.offset..end).context("truncated XDR")?;
        self.offset = end;
        Ok(u32::from_be_bytes(b.try_into()?))
    }
    fn opaque(&mut self, max: usize) -> Result<()> {
        let len = self.word()? as usize;
        ensure!(len <= max, "oversized XDR opaque");
        let end = self
            .offset
            .checked_add(len.div_ceil(4) * 4)
            .context("XDR overflow")?;
        ensure!(end <= self.bytes.len(), "truncated XDR opaque");
        self.offset = end;
        Ok(())
    }
    fn end(&self) -> Result<()> {
        ensure!(self.offset == self.bytes.len(), "trailing RPC data");
        Ok(())
    }
}
fn envelope(bytes: &[u8], xid: u32) -> Result<(Xdr<'_>, Option<&'static str>)> {
    ensure!(bytes.len() <= 4096, "oversized RPC response");
    let mut r = Xdr { bytes, offset: 0 };
    ensure!(
        r.word()? == xid && r.word()? == 1,
        "wrong RPC transaction or direction"
    );
    let status = match r.word()? {
        0 => {
            let _flavor = r.word()?;
            r.opaque(400)?;
            match r.word()? {
                0 => None,
                1 | 3 => Some("unsupported"),
                2 => {
                    r.word()?;
                    r.word()?;
                    Some("unsupported")
                }
                4 => Some("parse_error"),
                5 => Some("unavailable"),
                _ => bail!("invalid RPC accept status"),
            }
        }
        1 => match r.word()? {
            0 => {
                r.word()?;
                r.word()?;
                Some("unsupported")
            }
            1 => {
                r.word()?;
                Some("permission_denied")
            }
            _ => bail!("invalid RPC reject status"),
        },
        _ => bail!("invalid RPC reply status"),
    };
    if status.is_some() {
        r.end()?;
    }
    Ok((r, status))
}
/// Strict decoder; all normalized integers remain decimal strings.
pub fn decode_reply(bytes: &[u8], xid: u32) -> Result<Value> {
    let (mut r, error) = envelope(bytes, xid)?;
    if let Some(status) = error {
        return Ok(json!({"status":status}));
    }
    let status = r.word()?;
    if status == 2 || status == 3 {
        r.end()?;
        return Ok(json!({"status":if status==2{"no_quota"}else{"permission_denied"}}));
    }
    ensure!(status == 1, "invalid rquota status");
    let block = r.word()?;
    ensure!(
        block > 0 && block <= i32::MAX as u32,
        "invalid signed rquota block size"
    );
    let active = r.word()?;
    ensure!(active <= 1, "invalid XDR boolean");
    let hard = r.word()?;
    let soft = r.word()?;
    let used = r.word()?;
    let ihard = r.word()?;
    let isoft = r.word()?;
    let iused = r.word()?;
    let bgrace = r.word()?;
    let igrace = r.word()?;
    r.end()?;
    let bytes = |v: u32| -> Result<String> {
        Ok(u128::from(v)
            .checked_mul(u128::from(block))
            .context("quota byte overflow")?
            .to_string())
    };
    Ok(
        json!({"status":"available","block_size_bytes":block.to_string(),"active":active==1,"used_bytes":bytes(used)?,"block_hard_limit_bytes":bytes(hard)?,"block_soft_limit_bytes":bytes(soft)?,"used_inodes":iused.to_string(),"inode_hard_limit":ihard.to_string(),"inode_soft_limit":isoft.to_string(),"block_grace_seconds_raw":bgrace.to_string(),"inode_grace_seconds_raw":igrace.to_string(),"block_grace_seconds_signed":(bgrace as i32).to_string(),"inode_grace_seconds_signed":(igrace as i32).to_string()}),
    )
}
fn word(out: &mut Vec<u8>, v: u32) {
    out.extend(v.to_be_bytes());
}
fn opaque(out: &mut Vec<u8>, b: &[u8]) {
    word(out, b.len() as u32);
    out.extend(b);
    while out.len() % 4 != 0 {
        out.push(0);
    }
}
fn call(
    xid: u32,
    program: u32,
    procedure: u32,
    identity: &crate::platform::RpcIdentity,
) -> Vec<u8> {
    let mut out = Vec::new();
    for x in [
        xid,
        0,
        2,
        program,
        if program == 100000 { 2 } else { 1 },
        procedure,
    ] {
        word(&mut out, x);
    }
    let mut auth = Vec::new();
    word(&mut auth, chrono::Utc::now().timestamp() as u32);
    opaque(&mut auth, identity.hostname.as_bytes());
    word(&mut auth, identity.uid);
    word(&mut auth, identity.gid);
    word(&mut auth, identity.groups.len() as u32);
    for g in &identity.groups {
        word(&mut auth, *g);
    }
    word(&mut out, 1);
    opaque(&mut out, &auth);
    word(&mut out, 0);
    word(&mut out, 0);
    out
}
fn exchange(
    address: std::net::SocketAddr,
    request: &[u8],
    deadline: Instant,
) -> std::io::Result<Vec<u8>> {
    let socket = UdpSocket::bind(if address.is_ipv4() {
        "0.0.0.0:0"
    } else {
        "[::]:0"
    })?;
    socket.connect(address)?;
    let remaining = deadline
        .checked_duration_since(Instant::now())
        .ok_or(std::io::ErrorKind::TimedOut)?;
    socket.set_read_timeout(Some(remaining))?;
    socket.set_write_timeout(Some(remaining))?;
    socket.send(request)?;
    let mut bytes = [0u8; 4097];
    let n = socket.recv(&mut bytes)?;
    Ok(bytes[..n].to_vec())
}
fn network_status(error: std::io::Error) -> Value {
    json!({"status":match error.kind(){std::io::ErrorKind::TimedOut|std::io::ErrorKind::WouldBlock=>"timeout",std::io::ErrorKind::PermissionDenied=>"permission_denied",_=>"unavailable"}})
}
/// One explicitly configured server/UID. DNS is also bounded by the outer worker deadline.
pub fn query(target: &QuotaTarget, timeout: Duration) -> Value {
    if target.validate().is_err() || timeout.is_zero() || timeout > Duration::from_secs(10) {
        return json!({"status":"parse_error"});
    }
    let identity = match crate::platform::rpc_identity() {
        Ok(v) => v,
        Err(_) => return json!({"status":"unavailable"}),
    };
    let mut result = query_inner(target, timeout, &identity);
    result["query_identity_uid"] = identity.uid.to_string().into();
    result["query_identity_gid"] = identity.gid.to_string().into();
    result["query_groups_truncated"] = identity.groups_truncated.into();
    result
}
fn query_inner(
    target: &QuotaTarget,
    timeout: Duration,
    identity: &crate::platform::RpcIdentity,
) -> Value {
    let deadline = Instant::now() + timeout;
    let mut address = match (target.server.as_str(), target.port.unwrap_or(111)).to_socket_addrs() {
        Ok(mut a) => match a.next() {
            Some(a) => a,
            None => return json!({"status":"unavailable"}),
        },
        Err(e) => return network_status(e),
    };
    if target.port.is_none() {
        let xid = u32::from_be_bytes(uuid::Uuid::new_v4().as_bytes()[..4].try_into().unwrap());
        let mut request = call(xid, 100000, 3, identity);
        for x in [100011, 1, 17, 0] {
            word(&mut request, x);
        }
        let bytes = match exchange(address, &request, deadline) {
            Ok(b) => b,
            Err(e) => return network_status(e),
        };
        let registration = match decode_portmapper(&bytes, xid) {
            Ok(value) => value,
            Err(_) => return json!({"status":"parse_error"}),
        };
        if registration["status"] != "available" {
            return registration;
        }
        address.set_port(registration["port"].as_u64().expect("validated UDP port") as u16);
    }
    let xid = u32::from_be_bytes(uuid::Uuid::new_v4().as_bytes()[..4].try_into().unwrap());
    let mut request = call(xid, 100011, 1, identity);
    opaque(&mut request, target.export_path.as_bytes());
    word(&mut request, target.uid);
    match exchange(address, &request, deadline) {
        Ok(bytes) => decode_reply(&bytes, xid).unwrap_or_else(|_| json!({"status":"parse_error"})),
        Err(e) => network_status(e),
    }
}

/// Decode only the registered UDP port; RPC rejection remains its own observation.
pub fn decode_portmapper(bytes: &[u8], xid: u32) -> Result<Value> {
    let (mut r, status) = envelope(bytes, xid)?;
    if let Some(status) = status {
        return Ok(json!({"status":status}));
    }
    let port = u16::try_from(r.word()?).context("invalid portmapper UDP port")?;
    r.end()?;
    Ok(if port == 0 {
        json!({"status":"unsupported"})
    } else {
        json!({"status":"available","port":port})
    })
}
