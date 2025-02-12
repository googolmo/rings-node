#![warn(missing_docs)]
//! Processor of rings-node jsonrpc-server.
use std::str::FromStr;
use std::sync::Arc;

#[cfg(feature = "client")]
use jsonrpc_core::Metadata;

use crate::error::Error;
use crate::error::Result;
use crate::jsonrpc::method;
use crate::jsonrpc::response::TransportAndIce;
use crate::jsonrpc_client::SimpleClient;
use crate::prelude::rings_core::dht::Stabilization;
use crate::prelude::rings_core::message::Encoded;
use crate::prelude::rings_core::message::Message;
use crate::prelude::rings_core::message::MessageHandler;
use crate::prelude::rings_core::message::PayloadSender;
use crate::prelude::rings_core::prelude::uuid;
use crate::prelude::rings_core::prelude::web3::contract::tokens::Tokenizable;
use crate::prelude::rings_core::prelude::web3::ethabi::Token;
use crate::prelude::rings_core::prelude::web3::types::Address;
use crate::prelude::rings_core::prelude::RTCSdpType;
use crate::prelude::rings_core::swarm::Swarm;
use crate::prelude::rings_core::swarm::TransportManager;
use crate::prelude::rings_core::transports::Transport;
use crate::prelude::rings_core::types::ice_transport::IceTransport;
use crate::prelude::rings_core::types::ice_transport::IceTrickleScheme;

/// Processor for rings-node jsonrpc server
#[derive(Clone)]
pub struct Processor {
    /// a swarm instance
    pub swarm: Arc<Swarm>,
    /// a msg_handler instance
    pub msg_handler: Arc<MessageHandler>,
    /// a stabilization instane,
    pub stabilization: Arc<Stabilization>,
}

#[cfg(feature = "client")]
impl Metadata for Processor {}

impl From<(Arc<Swarm>, Arc<MessageHandler>, Arc<Stabilization>)> for Processor {
    fn from(
        (swarm, msg_handler, stabilization): (Arc<Swarm>, Arc<MessageHandler>, Arc<Stabilization>),
    ) -> Self {
        Self {
            swarm,
            msg_handler,
            stabilization,
        }
    }
}

impl Processor {
    /// Get current address
    pub fn address(&self) -> Address {
        self.swarm.address()
    }

    /// Create an Offer and waiting for connection.
    /// The process of manually handshake is:
    /// 1. PeerA: create_offer
    /// 2. PeerA: send the handshake info to PeerB.
    /// 3. PeerB: answer_offer
    /// 4. PeerB: send the handshake info to PeerA.
    /// 5. PeerA: accept_answer.
    pub async fn create_offer(&self) -> Result<(Arc<Transport>, Encoded)> {
        let transport = self
            .swarm
            .new_transport()
            .await
            .map_err(|_| Error::NewTransportError)?;
        let transport_cloned = transport.clone();
        let task = async move {
            let hs_info = transport_cloned
                .get_handshake_info(self.swarm.session_manager(), RTCSdpType::Offer)
                .await
                .map_err(Error::CreateOffer)?;
            self.swarm
                .push_pending_transport(&transport_cloned)
                .map_err(Error::PendingTransport)?;
            Ok(hs_info)
        };
        let hs_info = match task.await {
            Ok(hs_info) => (transport, hs_info),
            Err(e) => {
                transport.close().await.ok();
                return Err(e);
            }
        };
        Ok(hs_info)
    }

    /// Connect peer with remote rings-node jsonrpc server.
    /// * peer_url: the remote rings-node jsonrpc server url.
    pub async fn connect_peer_via_http(&self, peer_url: &str) -> Result<Arc<Transport>> {
        // request remote offer and sand answer to remote
        log::debug!("connect_peer_via_http: {}", peer_url);
        let transport = self
            .swarm
            .new_transport()
            .await
            .map_err(|_| Error::NewTransportError)?;
        let hs_info = self.do_connect_peer_via_http(&transport, peer_url).await;
        if let Err(e) = hs_info {
            transport
                .close()
                .await
                .map_err(Error::CloseTransportError)?;
            return Err(e);
        }
        Ok(transport)
    }

    async fn do_connect_peer_via_http(
        &self,
        transport: &Arc<Transport>,
        node_url: &str,
    ) -> Result<String> {
        let client = SimpleClient::new_with_url(node_url);
        let hs_info = transport
            .get_handshake_info(self.swarm.session_manager(), RTCSdpType::Offer)
            .await
            .map_err(Error::CreateOffer)?
            .to_string();
        log::debug!(
            "sending offer and candidate {:?} to {:?}",
            hs_info.to_owned(),
            node_url,
        );
        let resp = client
            .call_method(
                method::Method::AnswerOffer.as_str(),
                jsonrpc_core::Params::Array(vec![serde_json::json!(hs_info)]),
            )
            .await
            .map_err(|e| Error::RemoteRpcError(e.to_string()))?;
        let info: TransportAndIce =
            serde_json::from_value(resp).map_err(|_| Error::JsonDeserializeError)?;
        let addr = transport
            .register_remote_info(Encoded::from_encoded_str(info.ice.as_str()))
            .await
            .map_err(Error::RegisterIceError)?;
        // transport
        //     .connect_success_promise()
        //     .await
        //     .map_err(Error::ConnectError)?
        //     .await
        //     .map_err(Error::ConnectError)?;
        self.swarm
            .register(&addr, Arc::clone(transport))
            .await
            .map_err(Error::RegisterIceError)?;
        Ok(addr.to_string())
    }

    /// Answer an Offer.
    /// The process of manually handshake is:
    /// 1. PeerA: create_offer
    /// 2. PeerA: send the handshake info to PeerB.
    /// 3. PeerB: answer_offer
    /// 4. PeerB: send the handshake info to PeerA.
    /// 5. PeerA: accept_answer.
    pub async fn answer_offer(&self, ice_info: &str) -> Result<(Arc<Transport>, Encoded)> {
        log::info!("connect peer via ice: {}", ice_info);
        let transport = self.swarm.new_transport().await.map_err(|e| {
            log::error!("new_transport failed: {}", e);
            Error::NewTransportError
        })?;
        match self.handshake(&transport, ice_info).await {
            Ok(v) => Ok((transport, v)),
            Err(e) => {
                transport
                    .close()
                    .await
                    .map_err(Error::CloseTransportError)?;
                Err(e)
            }
        }
    }

    /// Connect peer with web3 address.
    /// There are 3 peers: PeerA, PeerB, PeerC.
    /// 1. PeerA has a connection with PeerB.
    /// 2. PeerC has a connection with PeerB.
    /// 3. PeerC can connect PeerA with PeerA's web3 address.
    pub async fn connect_with_address(
        &self,
        address: &Address,
        wait_for_open: bool,
    ) -> Result<Peer> {
        let transport = self
            .msg_handler
            .connect(address)
            .await
            .map_err(Error::ConnectWithAddressError)?;
        log::debug!("wait for transport connected");
        if wait_for_open {
            transport
                .wait_for_data_channel_open()
                .await
                .map_err(Error::ConnectWithAddressError)?;
        }
        Ok(Peer::from((*address, transport)))
    }

    async fn handshake(&self, transport: &Arc<Transport>, data: &str) -> Result<Encoded> {
        // get offer from remote and send answer back
        let hs_info = Encoded::from_encoded_str(data);
        let addr = transport
            .register_remote_info(hs_info.to_owned())
            .await
            .map_err(Error::RegisterIceError)?;

        log::debug!("register: {}", addr);
        self.swarm
            .register(&addr, Arc::clone(transport))
            .await
            .map_err(Error::RegisterIceError)?;

        let hs_info = transport
            .get_handshake_info(self.swarm.session_manager(), RTCSdpType::Answer)
            .await
            .map_err(Error::CreateAnswer)?;
        log::debug!("answer hs_info: {:?}", hs_info);
        Ok(hs_info)
    }

    /// Accept an answer of a connection.
    /// The process of manually handshake is:
    /// 1. PeerA: create_offer
    /// 2. PeerA: send the handshake info to PeerB.
    /// 3. PeerB: answer_offer
    /// 4. PeerB: send the handshake info to PeerA.
    /// 5. PeerA: accept_answer.
    pub async fn accept_answer(&self, transport_id: &str, ice: &str) -> Result<Peer> {
        let ice = Encoded::from_encoded_str(ice);
        log::debug!("accept_answer/ice: {:?}, uuid: {}", ice, transport_id);
        let transport_id =
            uuid::Uuid::from_str(transport_id).map_err(|_| Error::InvalidTransportId)?;
        let transport = self
            .swarm
            .find_pending_transport(transport_id)
            .map_err(Error::PendingTransport)?
            .ok_or(Error::TransportNotFound)?;
        let addr = transport
            .register_remote_info(ice)
            .await
            .map_err(Error::RegisterIceError)?;
        self.swarm
            .register(&addr, transport.clone())
            .await
            .map_err(Error::RegisterIceError)?;
        if let Err(e) = self.swarm.pop_pending_transport(transport.id) {
            log::warn!("pop_pending_transport err: {}", e)
        };
        Ok(Peer::from((addr, transport)))
    }

    /// List all peers.
    pub async fn list_peers(&self) -> Result<Vec<Peer>> {
        let transports = self.swarm.get_transports();
        log::debug!(
            "addresses: {:?}",
            transports.iter().map(|(a, _b)| a).collect::<Vec<_>>()
        );
        let data = transports.iter().map(|x| x.into()).collect::<Vec<Peer>>();
        Ok(data)
    }

    /// Get peer by remote address
    pub async fn get_peer(&self, address: &str) -> Result<Peer> {
        let address = Address::from_str(address).map_err(|_| Error::InvalidAddress)?;
        let transport = self
            .swarm
            .get_transport(&address)
            .ok_or(Error::TransportNotFound)?;
        Ok(Peer::from(&(address, transport)))
    }

    /// Disconnect a peer with web3 address.
    pub async fn disconnect(&self, address: &str) -> Result<()> {
        let address = Address::from_str(address).map_err(|_| Error::InvalidAddress)?;
        let transport = self
            .swarm
            .get_transport(&address)
            .ok_or(Error::TransportNotFound)?;
        transport
            .close()
            .await
            .map_err(Error::CloseTransportError)?;
        self.swarm.remove_transport(&address);
        Ok(())
    }

    /// List all pending transport.
    pub async fn list_pendings(&self) -> Result<Vec<Arc<Transport>>> {
        let pendings = self
            .swarm
            .pending_transports()
            .await
            .map_err(|_| Error::InternalError)?;
        Ok(pendings)
    }

    /// Close pending transport
    pub async fn close_pending_transport(&self, transport_id: &str) -> Result<()> {
        let transport_id =
            uuid::Uuid::from_str(transport_id).map_err(|_| Error::InvalidTransportId)?;
        let transport = self
            .swarm
            .find_pending_transport(transport_id)
            .map_err(|_| Error::TransportNotFound)?
            .ok_or(Error::TransportNotFound)?;
        if transport.is_connected().await {
            transport
                .close()
                .await
                .map_err(Error::CloseTransportError)?;
        }
        self.swarm
            .pop_pending_transport(transport_id)
            .map_err(Error::CloseTransportError)?;
        Ok(())
    }

    /// Send custom message to an address.
    pub async fn send_message(&self, destination: &str, msg: &[u8]) -> Result<()> {
        log::info!(
            "send_message, destination: {}, text: {:?}",
            destination,
            msg,
        );
        let destination = Address::from_str(destination).map_err(|_| Error::InvalidAddress)?;
        let msg = Message::custom(msg, &None).map_err(Error::SendMessage)?;
        // self.swarm.do_send_payload(address, payload)
        self.swarm
            .send_direct_message(msg, destination.into())
            .await
            .map_err(Error::SendMessage)?;
        Ok(())
    }
}

/// Peer struct
#[derive(Clone)]
pub struct Peer {
    /// web3 address of a peer.
    pub address: Token,
    /// transport of the connection.
    pub transport: Arc<Transport>,
}

impl From<(Address, Arc<Transport>)> for Peer {
    fn from((address, transport): (Address, Arc<Transport>)) -> Self {
        Self {
            address: address.into_token(),
            transport,
        }
    }
}

impl From<&(Address, Arc<Transport>)> for Peer {
    fn from((address, transport): &(Address, Arc<Transport>)) -> Self {
        Self {
            address: address.into_token(),
            transport: transport.clone(),
        }
    }
}

#[cfg(test)]
#[cfg(feature = "client")]
mod test {
    use futures::lock::Mutex;

    use super::*;
    use crate::prelude::*;

    fn new_processor() -> Processor {
        let key = SecretKey::random();

        let (auth, new_key) = SessionManager::gen_unsign_info(key.address(), None, None).unwrap();
        let sig = key.sign(&auth.to_string().unwrap()).to_vec();
        let session = SessionManager::new(&sig, &auth, &new_key);
        let swarm = Arc::new(Swarm::new(
            "stun://stun.l.google.com:19302",
            key.address(),
            session,
        ));

        let dht = Arc::new(Mutex::new(PeerRing::new(key.address().into())));
        let msg_handler = MessageHandler::new(dht.clone(), swarm.clone());
        let stabilization = Stabilization::new(dht, swarm.clone(), 200);
        (swarm, Arc::new(msg_handler), Arc::new(stabilization)).into()
    }

    #[tokio::test]
    async fn test_processor_create_offer() {
        let processor = new_processor();
        let ti = processor.create_offer().await.unwrap();
        let pendings = processor.swarm.pending_transports().await.unwrap();
        assert_eq!(pendings.len(), 1);
        assert_eq!(pendings.get(0).unwrap().id.to_string(), ti.0.id.to_string());
    }

    #[tokio::test]
    async fn test_processor_list_pendings() {
        let processor = new_processor();
        let ti0 = processor.create_offer().await.unwrap();
        let ti1 = processor.create_offer().await.unwrap();
        let pendings = processor.swarm.pending_transports().await.unwrap();
        assert_eq!(pendings.len(), 2);
        let pending_ids = processor.list_pendings().await.unwrap();
        assert_eq!(pendings.len(), pending_ids.len());
        let ids = vec![ti0.0.id.to_string(), ti1.0.id.to_string()];
        for item in pending_ids {
            assert!(
                ids.contains(&item.id.to_string()),
                "id[{}] not in list",
                item.id
            );
        }
    }

    #[tokio::test]
    async fn test_processor_close_pending_transport() {
        let processor = new_processor();
        let ti0 = processor.create_offer().await.unwrap();
        let _ti1 = processor.create_offer().await.unwrap();
        let ti2 = processor.create_offer().await.unwrap();
        let pendings = processor.swarm.pending_transports().await.unwrap();
        assert_eq!(pendings.len(), 3);
        assert!(
            processor.close_pending_transport("abc").await.is_err(),
            "close_pending_transport() should be error"
        );
        let transport1 = processor
            .swarm
            .find_pending_transport(uuid::Uuid::from_str(ti0.0.id.to_string().as_str()).unwrap())
            .unwrap();
        assert!(transport1.is_some(), "transport_1 should be Some()");
        let transport1 = transport1.unwrap();
        assert!(
            processor
                .close_pending_transport(ti0.0.id.to_string().as_str())
                .await
                .is_ok(),
            "close_pending_transport({}) should be ok",
            ti0.0.id
        );
        assert!(!transport1.is_connected().await, "transport1 should closed");

        let pendings = processor.swarm.pending_transports().await.unwrap();
        assert_eq!(pendings.len(), 2);

        assert!(
            !pendings
                .iter()
                .any(|x| x.id.to_string() == ti0.0.id.to_string()),
            "transport[{}] should not in pending_transports",
            ti0.0.id
        );

        let transport2 = processor
            .swarm
            .find_pending_transport(uuid::Uuid::from_str(ti2.0.id.to_string().as_str()).unwrap())
            .unwrap();
        assert!(transport2.is_some(), "transport2 should be Some()");
        let transport2 = transport2.unwrap();
        assert!(
            processor
                .close_pending_transport(ti2.0.id.to_string().as_str())
                .await
                .is_ok(),
            "close_pending_transport({}) should be ok",
            ti0.0.id
        );
        assert!(!transport2.is_connected().await, "transport2 should closed");

        let pendings = processor.swarm.pending_transports().await.unwrap();
        assert_eq!(pendings.len(), 1);

        assert!(
            !pendings
                .iter()
                .any(|x| x.id.to_string() == ti2.0.id.to_string()),
            "transport[{}] should not in pending_transports",
            ti0.0.id
        );
    }

    struct MsgCallbackStruct {
        msgs: Arc<Mutex<Vec<String>>>,
    }

    #[async_trait]
    impl MessageCallback for MsgCallbackStruct {
        async fn custom_message(
            &self,
            handler: &MessageHandler,
            _ctx: &MessagePayload<Message>,
            msg: &MaybeEncrypted<CustomMessage>,
        ) {
            let msg = handler.decrypt_msg(msg).unwrap();
            let text = String::from_utf8(msg.0).unwrap();
            let mut msgs = self.msgs.try_lock().unwrap();
            msgs.push(text);
        }

        async fn builtin_message(&self, _handler: &MessageHandler, _ctx: &MessagePayload<Message>) {
        }
    }

    #[tokio::test]
    async fn test_processor_handshake_msg() {
        let p1 = new_processor();
        let p2 = new_processor();
        let p1_addr = p1.address().into_token().to_string();
        let p2_addr = p2.address().into_token().to_string();
        println!("p1_addr: {}", p1_addr);
        println!("p2_addr: {}", p2_addr);

        let (transport_1, offer) = p1.create_offer().await.unwrap();

        let pendings_1 = p1.swarm.pending_transports().await.unwrap();
        assert_eq!(pendings_1.len(), 1);
        assert_eq!(
            pendings_1.get(0).unwrap().id.to_string(),
            transport_1.id.to_string()
        );

        let (transport_2, answer) = p2.answer_offer(offer.as_str()).await.unwrap();
        let peer = p1
            .accept_answer(transport_1.id.to_string().as_str(), answer.as_str())
            .await
            .unwrap();

        assert!(peer.transport.id.eq(&transport_1.id), "transport not same");
        assert!(
            peer.address.to_string().eq(&p2_addr),
            "peer.address got {}, expect: {}",
            peer.address,
            p2_addr
        );
        println!("waiting for connection");
        transport_1
            .connect_success_promise()
            .await
            .unwrap()
            .await
            .unwrap();
        transport_2
            .connect_success_promise()
            .await
            .unwrap()
            .await
            .unwrap();

        assert!(
            transport_1.is_connected().await,
            "transport_1 not connected"
        );
        assert!(
            p1.swarm
                .get_transport(&p2.address())
                .unwrap()
                .is_connected()
                .await,
            "p1 transport not connected"
        );
        assert!(
            transport_2.is_connected().await,
            "transport_2 not connected"
        );
        assert!(
            p2.swarm
                .get_transport(&p1.address())
                .unwrap()
                .is_connected()
                .await,
            "p2 transport not connected"
        );

        let msgs1: Arc<Mutex<Vec<String>>> = Default::default();
        let msgs2: Arc<Mutex<Vec<String>>> = Default::default();
        let callback1 = Box::new(MsgCallbackStruct {
            msgs: msgs1.clone(),
        });
        let callback2 = Box::new(MsgCallbackStruct {
            msgs: msgs2.clone(),
        });

        let msg_handler_1 = p1.msg_handler.clone();
        msg_handler_1.clone().set_callback(callback1).await;
        let msg_handler_2 = p2.msg_handler.clone();
        msg_handler_2.clone().set_callback(callback2).await;
        // tokio::spawn(async move {
        //     tokio::join!(
        //         async {
        //             msg_handler_1.clone().listen().await;
        //         },
        //         async {
        //             msg_handler_2.clone().listen().await;
        //         }
        //     );
        // });
        let test_text1 = "test1";
        let test_text2 = "test2";

        println!("send_message 1");
        p1.send_message(p2_addr.as_str(), test_text1.as_bytes())
            .await
            .unwrap();
        println!("send_message 1 done");

        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;

        println!("send_message 2");
        p2.send_message(p1_addr.as_str(), test_text2.as_bytes())
            .await
            .unwrap();
        println!("send_message 2 done");

        tokio::spawn(async move {
            tokio::join!(
                async {
                    msg_handler_1.clone().listen().await;
                },
                async {
                    msg_handler_2.clone().listen().await;
                }
            );
        });

        tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;

        println!("check received");

        let mut msgs2 = msgs2.try_lock().unwrap();
        let got_msg2 = msgs2.pop().unwrap();
        assert!(
            got_msg2.eq(test_text1),
            "msg received, expect {}, got {}",
            test_text1,
            got_msg2
        );

        let mut msgs1 = msgs1.try_lock().unwrap();
        let got_msg1 = msgs1.pop().unwrap();
        assert!(
            got_msg1.eq(test_text2),
            "msg received, expect {}, got {}",
            test_text2,
            got_msg1
        );
    }
}
