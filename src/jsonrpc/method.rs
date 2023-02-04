//! A JSONRPC `method` enum.
#![warn(missing_docs)]
use crate::error::Error;
use crate::error::Result;

/// supported methods.
#[derive(Debug, Clone)]
pub enum Method {
    /// Connect peer with remote jsonrpc server url
    ConnectPeerViaHttp,
    /// Connect peer with remote peer's did
    ConnectWithDid,
    /// Connect peers from a seed file
    ConnectWithSeed,
    /// List all connected peers
    ListPeers,
    /// Create offer for manually handshake
    CreateOffer,
    /// Answer offer for manually handshake
    AnswerOffer,
    /// Accept Answer for manually handshake
    AcceptAnswer,
    /// Send custom message to peer
    SendTo,
    /// Disconnect a peer
    Disconnect,
    /// List all pending connections
    ListPendings,
    /// Close pending connect
    ClosePendingTransport,
    /// Send ipfs request message
    SendHttpRequest,
    /// Send simple text message
    SendSimpleText,
    /// Append data to topic
    PublishMessageToTopic,
    /// Fetch data of topic
    FetchMessagesOfTopic,
    /// Register service
    RegisterService,
    /// Lookup service
    LookupService,
    /// Poll message
    PollMessage,
}

impl Method {
    /// Return method's name as `&str`
    pub fn as_str(&self) -> &str {
        match self {
            Method::ConnectPeerViaHttp => "connectPeerViaHttp",
            Method::ConnectWithDid => "connectWithDid",
            Method::ConnectWithSeed => "connectWithSeed",
            Method::ListPeers => "listPeers",
            Method::CreateOffer => "createOffer",
            Method::AnswerOffer => "answerOffer",
            Method::SendTo => "sendTo",
            Method::Disconnect => "disconnect",
            Method::AcceptAnswer => "acceptAnswer",
            Method::ListPendings => "listPendings",
            Method::ClosePendingTransport => "closePendingTransport",
            Method::SendHttpRequest => "sendHttpRequest",
            Method::SendSimpleText => "sendSimpleText",
            Method::PublishMessageToTopic => "publishMessageToTopic",
            Method::FetchMessagesOfTopic => "fetchMessagesOfTopic",
            Method::RegisterService => "registerService",
            Method::LookupService => "lookupService",
            Method::PollMessage => "pollMessage",
        }
    }
}

impl ToString for Method {
    fn to_string(&self) -> String {
        self.as_str().to_owned()
    }
}

impl TryFrom<&str> for Method {
    type Error = crate::error::Error;

    fn try_from(value: &str) -> Result<Self> {
        Ok(match value {
            "connectPeerViaHttp" => Self::ConnectPeerViaHttp,
            "connectWithDid" => Self::ConnectWithDid,
            "connectWithSeed" => Self::ConnectWithSeed,
            "listPeers" => Self::ListPeers,
            "createOffer" => Self::CreateOffer,
            "answerOffer" => Self::AnswerOffer,
            "sendTo" => Self::SendTo,
            "disconnect" => Self::Disconnect,
            "acceptAnswer" => Self::AcceptAnswer,
            "listPendings" => Self::ListPendings,
            "closePendingTransport" => Self::ClosePendingTransport,
            "sendHttpRequest" => Self::SendHttpRequest,
            "sendSimpleText" => Self::SendSimpleText,
            "publishMessageToTopic" => Method::PublishMessageToTopic,
            "fetchMessagesOfTopic" => Method::FetchMessagesOfTopic,
            "registerService" => Method::RegisterService,
            "lookupService" => Method::LookupService,
            "pollMessage" => Method::PollMessage,
            _ => return Err(Error::InvalidMethod),
        })
    }
}
