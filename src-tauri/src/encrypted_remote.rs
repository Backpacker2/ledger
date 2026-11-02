//! A [`Remote`] that seals everything it uploads and opens everything it downloads, around whichever remote does
//! the actual talking (GitHub, or the fake in the tests). Paths stay readable: only file contents are encrypted.

use std::collections::BTreeMap;

use crate::crypto::{self, CryptoError, Key};
use crate::error::{Error, Result};
use crate::sync::{Change, Head, Remote};

pub struct EncryptedRemote<R> {
    inner: R,
    key: Key,
}

impl<R: Remote> EncryptedRemote<R> {
    pub fn new(inner: R, key: Key) -> EncryptedRemote<R> {
        EncryptedRemote { inner, key }
    }
}

fn failed(path: &str, cause: CryptoError) -> Error {
    Error::Crypto { path: path.to_string(), cause }
}

impl<R: Remote> Remote for EncryptedRemote<R> {
    fn head(&self) -> Result<Option<Head>> {
        self.inner.head()
    }

    fn tree(&self, head: &Head) -> Result<BTreeMap<String, String>> {
        self.inner.tree(head)
    }

    fn blob(&self, path: &str, sha: &str) -> Result<Vec<u8>> {
        let stored = self.inner.blob(path, sha)?;
        // A plain file is never taken in: a branch that holds one can't be an encrypted one.
        crypto::unarmor(&stored).and_then(|sealed| crypto::decrypt(&self.key, path, &sealed)).map_err(|cause| failed(path, cause))
    }

    fn push(&self, parent: Option<&Head>, changes: &[Change], message: &str) -> Result<Head> {
        let sealed = changes
            .iter()
            .map(|change| match change {
                Change::Put { path, bytes } => {
                    let blob = crypto::encrypt(&self.key, path, bytes).map_err(|cause| failed(path, cause))?;
                    Ok(Change::Put { path: path.clone(), bytes: crypto::armor(&blob) })
                }
                Change::Delete { path } => Ok(Change::Delete { path: path.clone() }),
            })
            .collect::<Result<Vec<Change>>>()?;
        self.inner.push(parent, &sealed, message)
    }

    fn key_id(&self) -> Option<u8> {
        Some(self.key.id())
    }
}
