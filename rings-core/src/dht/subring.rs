#![warn(missing_docs)]
use std::str::FromStr;

use serde::Deserialize;
use serde::Serialize;

use super::chord::PeerRing;
use super::chord::PeerRingAction;
use super::chord::RemoteAction;
use super::types::Chord;
use super::types::SubRingManager;
use super::vnode::VNodeType;
use super::vnode::VirtualNode;
use super::FingerTable;
use crate::dht::Did;
use crate::ecc::HashStr;
use crate::err::Error;
use crate::err::Result;

/// A SubRing is a full functional Ring, but with a name and it's finger table can be
/// stored on Main Rings DHT, For a SubRing, it's virtual address is `sha1(name)`
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubRing {
    /// name of subring
    pub name: String,
    /// did of subring, generate with hash(name)
    pub did: Did,
    /// finger table
    pub finger: FingerTable,
    /// admin of ring, for verify that a message is come from ring
    pub admin: Option<Did>,
    /// creator
    pub creator: Did,
}

impl SubRingManager<PeerRingAction> for PeerRing {
    fn join_subring(&self, id: &Did, rid: &Did) -> Result<PeerRingAction> {
        match self.find_successor(*rid) {
            Ok(PeerRingAction::Some(_)) => {
                let id = id.to_owned();
                self.get_subring_for_update(rid, box move |r: SubRing| {
                    let mut new_ring = r;
                    new_ring.finger.join(id);
                    new_ring
                })?;
                Ok(PeerRingAction::None)
            }
            Ok(PeerRingAction::RemoteAction(n, RemoteAction::FindSuccessor(_))) => Ok(
                PeerRingAction::RemoteAction(n, RemoteAction::FindAndJoinSubRing(*rid)),
            ),
            Ok(a) => Err(Error::PeerRingUnexpectedAction(a)),
            Err(e) => Err(e),
        }
    }

    fn cloest_preceding_node_for_subring(&self, id: &Did, rid: &Did) -> Option<Result<Did>> {
        let id = id.to_owned();
        if let Some(Ok(subring)) = self.get_subring(rid) {
            Some(subring.finger.closest(id))
        } else {
            None
        }
    }

    fn get_subring(&self, id: &Did) -> Option<Result<SubRing>> {
        self.storage.get(id).map(|vn| vn.try_into())
    }

    fn store_subring(&self, subring: &SubRing) -> Result<()> {
        let id = subring.did;
        self.storage.set(&id, subring.clone().try_into()?);
        Ok(())
    }

    fn get_subring_by_name(&self, name: &str) -> Option<Result<SubRing>> {
        let address: HashStr = name.to_owned().into();
        // trans Result to Option here
        let did = Did::from_str(&address.inner()).ok()?;
        self.get_subring(&did)
    }
    /// get subring, update and putback
    fn get_subring_for_update(
        &self,
        id: &Did,
        callback: Box<dyn FnOnce(SubRing) -> SubRing>,
    ) -> Result<bool> {
        if let Some(Ok(subring)) = self.get_subring(id) {
            let sr = callback(subring);
            self.store_subring(&sr)?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// get subring, update and putback
    fn get_subring_for_update_by_name(
        &self,
        name: &str,
        callback: Box<dyn FnOnce(SubRing) -> SubRing>,
    ) -> Result<bool> {
        let address: HashStr = name.to_owned().into();
        let did = Did::from_str(&address.inner())?;
        self.get_subring_for_update(&did, callback)
    }
}

impl SubRing {
    /// Create a new SubRing
    pub fn new(name: &str, creator: &Did) -> Result<Self> {
        let address: HashStr = name.to_owned().into();
        let did = Did::from_str(&address.inner())?;
        Ok(Self {
            name: name.to_owned(),
            did,
            finger: FingerTable::new(did, 1),
            admin: None,
            creator: *creator,
        })
    }

    /// Create a SubRing from Ring
    pub fn from_ring(name: &str, ring: &PeerRing) -> Result<Self> {
        let address: HashStr = name.to_owned().into();
        let did = Did::from_str(&address.inner())?;
        Ok(Self {
            name: name.to_owned(),
            did,
            finger: ring.finger.clone(),
            admin: None,
            creator: ring.id,
        })
    }
}

impl TryFrom<SubRing> for VirtualNode {
    type Error = Error;
    fn try_from(ring: SubRing) -> Result<Self> {
        let data = serde_json::to_string(&ring).map_err(|_| Error::SerializeToString)?;
        Ok(Self {
            address: ring.did,
            data: vec![data.into()],
            kind: VNodeType::SubRing,
        })
    }
}

impl TryFrom<VirtualNode> for SubRing {
    type Error = Error;
    fn try_from(vnode: VirtualNode) -> Result<Self> {
        match &vnode.kind {
            VNodeType::SubRing => {
                let decoded: String = vnode.data[0].decode()?;
                let subring: SubRing =
                    serde_json::from_str(&decoded).map_err(Error::Deserialize)?;
                Ok(subring)
            }
            _ => Err(Error::InvalidVNodeType),
        }
    }
}

impl From<SubRing> for PeerRing {
    fn from(ring: SubRing) -> Self {
        let mut pr = PeerRing::new_with_config(ring.did, 1);
        // set finger[0] to successor
        if let Some(id) = ring.finger.first() {
            pr.successor.update(id);
        }
        pr.finger = ring.finger;
        pr
    }
}
