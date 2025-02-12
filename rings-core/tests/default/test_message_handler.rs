#[cfg(test)]
pub mod test {
    use std::str::FromStr;
    use std::sync::Arc;

    use futures::lock::Mutex;
    use rings_core::dht::vnode::VirtualNode;
    use rings_core::dht::Did;
    use rings_core::dht::PeerRing;
    use rings_core::ecc::SecretKey;
    use rings_core::err::Result;
    use rings_core::message;
    use rings_core::message::Encoder;
    use rings_core::message::Message;
    use rings_core::message::MessageHandler;
    use rings_core::message::PayloadSender;
    use rings_core::session::SessionManager;
    use rings_core::swarm::Swarm;
    use rings_core::swarm::TransportManager;
    use rings_core::transports::Transport;
    use rings_core::types::ice_transport::IceTransport;
    use rings_core::types::ice_transport::IceTrickleScheme;
    use rings_core::types::message::MessageListener;
    use tokio::time::sleep;
    use tokio::time::Duration;
    use webrtc::ice_transport::ice_connection_state::RTCIceConnectionState;
    use webrtc::peer_connection::sdp::sdp_type::RTCSdpType;

    fn new_chord(did: Did) -> PeerRing {
        PeerRing::new(did)
    }

    fn new_swarm(key: &SecretKey) -> Swarm {
        let stun = "stun://stun.l.google.com:19302";
        let session = SessionManager::new_with_seckey(key).unwrap();
        Swarm::new(stun, key.address(), session)
    }

    pub async fn establish_connection(
        swarm1: Arc<Swarm>,
        swarm2: Arc<Swarm>,
    ) -> Result<(Arc<Transport>, Arc<Transport>)> {
        assert!(swarm1.get_transport(&swarm2.address()).is_none());
        assert!(swarm2.get_transport(&swarm1.address()).is_none());

        let transport1 = swarm1.new_transport().await.unwrap();
        let transport2 = swarm2.new_transport().await.unwrap();

        assert_eq!(
            transport1.ice_connection_state().await,
            Some(RTCIceConnectionState::New)
        );
        assert_eq!(
            transport2.ice_connection_state().await,
            Some(RTCIceConnectionState::New)
        );

        // Peer 1 try to connect peer 2
        let handshake_info1 = transport1
            .get_handshake_info(swarm1.session_manager(), RTCSdpType::Offer)
            .await?;
        assert_eq!(
            transport1.ice_connection_state().await,
            Some(RTCIceConnectionState::New)
        );
        assert_eq!(
            transport2.ice_connection_state().await,
            Some(RTCIceConnectionState::New)
        );

        // Peer 2 got offer then register
        let addr1 = transport2.register_remote_info(handshake_info1).await?;
        assert_eq!(addr1, swarm1.address());
        assert_eq!(
            transport1.ice_connection_state().await,
            Some(RTCIceConnectionState::New)
        );
        assert_eq!(
            transport2.ice_connection_state().await,
            Some(RTCIceConnectionState::New)
        );

        // Peer 2 create answer
        let handshake_info2 = transport2
            .get_handshake_info(swarm2.session_manager(), RTCSdpType::Answer)
            .await?;
        assert_eq!(
            transport1.ice_connection_state().await,
            Some(RTCIceConnectionState::New)
        );
        assert_eq!(
            transport2.ice_connection_state().await,
            Some(RTCIceConnectionState::Checking)
        );

        // Peer 1 got answer then register
        let addr2 = transport1.register_remote_info(handshake_info2).await?;
        assert_eq!(addr2, swarm2.address());
        let promise_1 = transport1.connect_success_promise().await?;
        let promise_2 = transport2.connect_success_promise().await?;
        promise_1.await?;
        promise_2.await?;
        assert_eq!(
            transport1.ice_connection_state().await,
            Some(RTCIceConnectionState::Connected)
        );
        assert_eq!(
            transport2.ice_connection_state().await,
            Some(RTCIceConnectionState::Connected)
        );
        swarm1
            .register(&swarm2.address(), transport1.clone())
            .await?;
        swarm2
            .register(&swarm1.address(), transport2.clone())
            .await?;
        let transport_1_to_2 = swarm1.get_transport(&swarm2.address()).unwrap();
        let transport_2_to_1 = swarm2.get_transport(&swarm1.address()).unwrap();

        assert!(Arc::ptr_eq(&transport_1_to_2, &transport1));
        assert!(Arc::ptr_eq(&transport_2_to_1, &transport2));

        Ok((transport1, transport2))
    }

    #[tokio::test]
    async fn test_handle_join() -> Result<()> {
        let key1 = SecretKey::random();
        let key2 = SecretKey::random();
        let dht1 = Arc::new(Mutex::new(new_chord(key1.address().into())));
        let swarm1 = Arc::new(new_swarm(&key1));
        let swarm2 = Arc::new(new_swarm(&key2));
        let (_, _) = establish_connection(Arc::clone(&swarm1), Arc::clone(&swarm2)).await?;
        let handle1 = MessageHandler::new(Arc::clone(&dht1), Arc::clone(&swarm1));
        let payload = swarm1.poll_message().await.unwrap();
        match handle1.handle_payload(&payload).await {
            Ok(_) => assert_eq!(true, true),
            Err(e) => {
                println!("{:?}", e);
                assert_eq!(true, false);
            }
        };
        assert!(dht1
            .lock()
            .await
            .successor
            .list()
            .contains(&key2.address().into()));
        Ok(())
    }

    #[tokio::test]
    async fn test_handle_connect_node() -> Result<()> {
        let mut key1 = SecretKey::random();
        let mut key2 = SecretKey::random();
        let mut key3 = SecretKey::random();

        let mut v = vec![key1, key2, key3];
        v.sort_by(|a, b| {
            if a.address() < b.address() {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Greater
            }
        });
        (key1, key2, key3) = (v[0], v[1], v[2]);
        assert!(key1.address() < key2.address(), "key1 < key2");
        assert!(key2.address() < key3.address(), "key2 < key3");
        assert!(key1.address() < key3.address(), "key1 < key3");
        let swarm1 = Arc::new(new_swarm(&key1));
        let swarm2 = Arc::new(new_swarm(&key2));
        let swarm3 = Arc::new(new_swarm(&key3));

        let dht1 = Arc::new(Mutex::new(new_chord(key1.address().into())));
        let dht2 = Arc::new(Mutex::new(new_chord(key2.address().into())));
        let dht3 = Arc::new(Mutex::new(new_chord(key3.address().into())));

        // 2 to 3
        let (_, _) = establish_connection(Arc::clone(&swarm3), Arc::clone(&swarm2)).await?;

        // 1 to 2
        let (_, _) = establish_connection(Arc::clone(&swarm1), Arc::clone(&swarm2)).await?;

        sleep(Duration::from_secs(3)).await;

        let handler1 = MessageHandler::new(Arc::clone(&dht1), Arc::clone(&swarm1));
        let handler2 = MessageHandler::new(Arc::clone(&dht2), Arc::clone(&swarm2));
        let handler3 = MessageHandler::new(Arc::clone(&dht3), Arc::clone(&swarm3));

        tokio::select! {
            _ = async {
                futures::join!(
                    async { Arc::new(handler1.clone()).listen().await },
                    async { Arc::new(handler2.clone()).listen().await },
                    async { Arc::new(handler3.clone()).listen().await },
                )
            } => {unreachable!();}
            _ = async {
                // handle join dht situation
                println!("wait tranposrt 1 to 2 and transport 2 to 3 connected");
                sleep(Duration::from_millis(1)).await;
                let transport_1_to_2 = swarm1.get_transport(&swarm2.address()).unwrap();
                transport_1_to_2.wait_for_data_channel_open().await.unwrap();
                let transport_2_to_3 = swarm2.get_transport(&swarm3.address()).unwrap();
                transport_2_to_3.wait_for_data_channel_open().await.unwrap();

                println!("wait events trigger");
                sleep(Duration::from_millis(1)).await;

                println!("swarm1 key address: {:?}", swarm1.address());
                println!("swarm2 key address: {:?}", swarm2.address());
                println!("swarm3 key address: {:?}", swarm3.address());
                let dht1_successor = {dht1.lock().await.successor.clone()};
                let dht2_successor = {dht2.lock().await.successor.clone()};
                let dht3_successor = {dht3.lock().await.successor.clone()};
                println!("dht1 successor: {:?}", dht1_successor);
                println!("dht2 successor: {:?}", dht2_successor);
                println!("dht3 successor: {:?}", dht3_successor);

                // key1 < key2 < key3
                // dht1 -> dht2
                // dht2 -> dht3

                // dht3 -> dht2
                assert!(
                    dht1_successor.list().contains(
                        &key2.address().into()
                    ),
                    "Expect dht1 successor is key2, Found: {:?}",
                    dht1_successor.list()
                );
                assert!(
                    dht2_successor.list().contains(
                        &key3.address().into()
                    ), "{:?}", dht2_successor.list());
                assert!(
                    dht3_successor.list().contains(
                        &key2.address().into()
                    ),
                    "dht3 successor is key2"
                );
                assert_eq!(
                    transport_1_to_2.ice_connection_state().await,
                    Some(RTCIceConnectionState::Connected)
                );
                assert_eq!(
                    transport_2_to_3.ice_connection_state().await,
                    Some(RTCIceConnectionState::Connected)
                );

                // dht1 send msg to dht2 ask for connecting dht3
                handler1.connect(&swarm3.address()).await.unwrap();
                sleep(Duration::from_millis(10000)).await;

                let transport_1_to_3 = swarm1.get_transport(&swarm3.address());
                assert!(transport_1_to_3.is_some());
                let transport_1_to_3 = transport_1_to_3.unwrap();
                let both = {
                    transport_1_to_3.ice_connection_state().await == Some(RTCIceConnectionState::New) ||
                        transport_1_to_3.ice_connection_state().await == Some(RTCIceConnectionState::Checking) ||
                        transport_1_to_3.ice_connection_state().await == Some(RTCIceConnectionState::Connected)
                };
                assert!(both, "{:?}", transport_1_to_3.ice_connection_state().await);
                transport_1_to_3.wait_for_data_channel_open().await.unwrap();
                assert_eq!(
                    transport_1_to_3.ice_connection_state().await,
                    Some(RTCIceConnectionState::Connected)
                );
            } => {}
        }
        Ok(())
    }

    #[tokio::test]
    async fn test_handle_notify_predecessor() -> Result<()> {
        let key1 = SecretKey::random();
        let key2 = SecretKey::random();
        let dht1 = Arc::new(Mutex::new(new_chord(key1.address().into())));
        let dht2 = Arc::new(Mutex::new(new_chord(key2.address().into())));
        let swarm1 = Arc::new(new_swarm(&key1));
        let swarm2 = Arc::new(new_swarm(&key2));
        let (_, _) = establish_connection(Arc::clone(&swarm1), Arc::clone(&swarm2)).await?;
        let handler1 = MessageHandler::new(Arc::clone(&dht1), Arc::clone(&swarm1));
        let handler2 = MessageHandler::new(Arc::clone(&dht2), Arc::clone(&swarm2));

        // handle join dht situation
        tokio::select! {
            _ = async {
                futures::join!(
                    async {
                        loop {
                            Arc::new(handler1.clone()).listen().await;
                        }
                    },
                    async {
                        loop {
                            Arc::new(handler2.clone()).listen().await;
                        }
                    }
                );
            } => { unreachable!();}
            _ = async {
                let transport_1_to_2 = swarm1.get_transport(&swarm2.address()).unwrap();
                sleep(Duration::from_millis(1000)).await;
                transport_1_to_2.wait_for_data_channel_open().await.unwrap();
                assert!(dht1.lock().await.successor.list().contains(&key2.address().into()));
                assert!(dht2.lock().await.successor.list().contains(&key1.address().into()));
                assert_eq!(
                    transport_1_to_2.ice_connection_state().await,
                    Some(RTCIceConnectionState::Connected)
                );
                handler1
                    .send_message(
                        Message::NotifyPredecessorSend(message::NotifyPredecessorSend {
                            id: key1.address().into(),
                        }),
                        swarm2.address().into(),
                        swarm2.address().into(),
                    )
                    .await
                    .unwrap();
                sleep(Duration::from_millis(1000)).await;
                assert_eq!(dht2.lock().await.predecessor, Some(key1.address().into()));
                assert!(dht1.lock().await.successor.list().contains(&key2.address().into()));
            } => {}
        }

        Ok(())
    }

    #[tokio::test]
    async fn test_handle_find_successor_increase() -> Result<()> {
        let mut key1 = SecretKey::random();
        let mut key2 = SecretKey::random();
        if key1.address() > key2.address() {
            (key1, key2) = (key2, key1)
        }
        let dht1 = Arc::new(Mutex::new(new_chord(key1.address().into())));
        let dht2 = Arc::new(Mutex::new(new_chord(key2.address().into())));
        let swarm1 = Arc::new(new_swarm(&key1));
        let swarm2 = Arc::new(new_swarm(&key2));
        let (_, _) = establish_connection(Arc::clone(&swarm1), Arc::clone(&swarm2)).await?;

        let handler1 = MessageHandler::new(Arc::clone(&dht1), Arc::clone(&swarm1));
        let handler2 = MessageHandler::new(Arc::clone(&dht2), Arc::clone(&swarm2));

        tokio::select! {
            _ = async {
                futures::join!(
                    async {
                        loop {
                            Arc::new(handler1.clone()).listen().await;
                        }
                    },
                    async {
                        loop {
                            Arc::new(handler2.clone()).listen().await;
                        }
                    }
                );
            } => { unreachable!();}
            _ = async {
                let transport_1_to_2 = swarm1.get_transport(&swarm2.address()).unwrap();
                sleep(Duration::from_millis(1000)).await;
                transport_1_to_2.wait_for_data_channel_open().await.unwrap();
                assert!(dht1.lock().await.successor.list().contains(&key2.address().into()), "{:?}", dht1.lock().await.successor.list());
                assert!(dht2.lock().await.successor.list().contains(&key1.address().into()));
                assert_eq!(
                    transport_1_to_2.ice_connection_state().await,
                    Some(RTCIceConnectionState::Connected)
                );
                handler1
                    .send_message(
                        Message::NotifyPredecessorSend(message::NotifyPredecessorSend {
                            id: swarm1.address().into(),
                        }),
                        swarm2.address().into(),
                        swarm2.address().into(),
                    )
                    .await
                    .unwrap();
                sleep(Duration::from_millis(1000)).await;
                assert_eq!(dht2.lock().await.predecessor, Some(key1.address().into()));
                assert!(dht1.lock().await.successor.list().contains(&key2.address().into()));

                println!(
                    "swarm1: {:?}, swarm2: {:?}",
                    swarm1.address(),
                    swarm2.address()
                );
                handler2
                    .send_message(
                        Message::FindSuccessorSend(message::FindSuccessorSend {
                            id: swarm2.address().into(),
                            for_fix: false,
                        }),
                        swarm1.address().into(),
                        swarm1.address().into(),
                    )
                    .await
                    .unwrap();
                sleep(Duration::from_millis(1000)).await;
                assert!(dht2.lock().await.successor.list().contains(&key1.address().into()));
                assert!(dht1.lock().await.successor.list().contains(&key2.address().into()));
            } => {}
        }
        Ok(())
    }

    #[tokio::test]
    async fn test_handle_find_successor_decrease() -> Result<()> {
        let mut key1 = SecretKey::random();
        let mut key2 = SecretKey::random();
        // key 2 > key 1 here
        if key1.address() < key2.address() {
            (key1, key2) = (key2, key1)
        }
        let dht1 = Arc::new(Mutex::new(new_chord(key1.address().into())));
        let dht2 = Arc::new(Mutex::new(new_chord(key2.address().into())));
        let swarm1 = Arc::new(new_swarm(&key1));
        let swarm2 = Arc::new(new_swarm(&key2));
        let (_, _) = establish_connection(Arc::clone(&swarm1), Arc::clone(&swarm2)).await?;

        let handler1 = MessageHandler::new(Arc::clone(&dht1), Arc::clone(&swarm1));
        let handler2 = MessageHandler::new(Arc::clone(&dht2), Arc::clone(&swarm2));

        // handle join dht situation
        tokio::select! {
            _ = async {
                futures::join!(
                    async {
                        loop {
                            Arc::new(handler1.clone()).listen().await;
                        }
                    },
                    async {
                        loop {
                            Arc::new(handler2.clone()).listen().await;
                        }
                    }
                );
            } => {unreachable!();}
            _ = async {
                let transport_1_to_2 = swarm1.get_transport(&swarm2.address()).unwrap();
                sleep(Duration::from_millis(1000)).await;
                transport_1_to_2.wait_for_data_channel_open().await.unwrap();
                assert!(dht1.lock().await.successor.list().contains(&key2.address().into()));
                assert!(dht2.lock().await.successor.list().contains(&key1.address().into()));
                assert!(dht1
                    .lock()
                    .await
                    .finger
                    .contains(&Some(key2.address().into())));
                assert!(dht2
                    .lock()
                    .await
                    .finger
                    .contains(&Some(key1.address().into())));
                assert_eq!(
                    transport_1_to_2.ice_connection_state().await,
                    Some(RTCIceConnectionState::Connected)
                );
                handler1
                    .send_message(
                        Message::NotifyPredecessorSend(message::NotifyPredecessorSend {
                            id: swarm1.address().into(),
                        }),
                        swarm2.address().into(),
                        swarm2.address().into(),
                    )
                    .await
                    .unwrap();
                sleep(Duration::from_millis(1000)).await;
                assert_eq!(dht2.lock().await.predecessor, Some(key1.address().into()));
                assert!(dht1.lock().await.successor.list().contains(&key2.address().into()));
                println!(
                    "swarm1: {:?}, swarm2: {:?}",
                    swarm1.address(),
                    swarm2.address()
                );
                handler2
                    .send_message(
                        Message::FindSuccessorSend(message::FindSuccessorSend {
                            id: swarm2.address().into(),
                            for_fix: false,
                        }),
                        swarm1.address().into(),
                        swarm1.address().into(),
                    )
                    .await
                    .unwrap();
                sleep(Duration::from_millis(1000)).await;
                let dht1_successor = dht1.lock().await.successor.clone();
                let dht2_successor = dht2.lock().await.successor.clone();
                assert!(dht2_successor.list().contains(&key1.address().into()));
                assert!(dht1_successor.list().contains(&key2.address().into()));
            } => {}
        };
        Ok(())
    }

    #[tokio::test]
    async fn test_handle_storage() -> Result<()> {
        // random key may faile here, because if key1 is more close to virtual_peer
        // key2 will try send msg back to key1
        let key1 =
            SecretKey::from_str("ff3e0ea83de6909db79f3452764a24efb25c86c1e85c7c453d903c0cf462df07")
                .unwrap();
        let key2 =
            SecretKey::from_str("f782f6b07ae0151b5f83ff49f46087a7a45eb5c97d210c907a2b52ffece4be69")
                .unwrap();
        let dht1 = Arc::new(Mutex::new(new_chord(key1.address().into())));
        let dht2 = Arc::new(Mutex::new(new_chord(key2.address().into())));
        println!(
            "test with key1: {:?}, key2: {:?}",
            key1.address(),
            key2.address()
        );
        let swarm1 = Arc::new(new_swarm(&key1));
        let swarm2 = Arc::new(new_swarm(&key2));
        let (_, _) = establish_connection(Arc::clone(&swarm1), Arc::clone(&swarm2)).await?;

        let handler1 = MessageHandler::new(Arc::clone(&dht1), Arc::clone(&swarm1));
        let handler2 = MessageHandler::new(Arc::clone(&dht2), Arc::clone(&swarm2));
        tokio::select! {
             _ = async {
                 futures::join!(
                     async {
                         loop {
                             Arc::new(handler1.clone()).listen().await;
                         }
                     },
                     async {
                         loop {
                             Arc::new(handler2.clone()).listen().await;
                         }
                     }
                 );
             } => { unreachable!();}
             _ = async {
                 let transport_1_to_2 = swarm1.get_transport(&swarm2.address()).unwrap();
                 sleep(Duration::from_millis(1000)).await;
                 transport_1_to_2.wait_for_data_channel_open().await.unwrap();
                 // dht1's successor is dht2
                 // dht2's successor is dht1
                 assert!(dht1.lock().await.successor.list().contains(&key2.address().into()));
                 assert!(dht2.lock().await.successor.list().contains(&key1.address().into()));
                 assert_eq!(
                     transport_1_to_2.ice_connection_state().await,
                     Some(RTCIceConnectionState::Connected)
                 );
                 handler1
                     .send_message(
                         Message::NotifyPredecessorSend(message::NotifyPredecessorSend {
                             id: swarm1.address().into(),
                         }),
                         swarm2.address().into(),
                         swarm2.address().into(),
                     )
                     .await
                     .unwrap();
                 sleep(Duration::from_millis(1000)).await;
                 assert_eq!(dht2.lock().await.predecessor, Some(key1.address().into()));
                 assert!(dht1.lock().await.successor.list().contains(&key2.address().into()));

                 assert!(dht2.lock().await.storage.len() == 0);
                 let message = String::from("this is a test string");
                 let encoded_message = message.encode().unwrap();
                 // the vid is hash of string
                 let vnode: VirtualNode = encoded_message.try_into().unwrap();
                 handler1.send_message(
                     Message::StoreVNode(message::StoreVNode {
                         data: vec![vnode.clone()]
                     }),
                     swarm2.address().into(),
                     swarm2.address().into(),
                 )
                 .await
                 .unwrap();
                 sleep(Duration::from_millis(5000)).await;
                 assert!(dht1.lock().await.storage.len() == 0);
                 assert!(dht2.lock().await.storage.len() > 0);
                 let data = dht2.lock().await.storage.get(&(vnode.address));
                 assert!(data.is_some(), "vnode: {:?} not in , exist keys {:?}",
                         vnode.did(),
                         dht2.lock().await.storage.keys());
                 let data = data.unwrap();
                 assert_eq!(data.data[0].clone().decode::<String>().unwrap(), message);
             } => {}
        }
        Ok(())
    }
}
