use error_chain::*;
use message::Message;
use tokio::sync::mpsc::error::{
    RecvError, UnboundedRecvError, UnboundedSendError, UnboundedTrySendError,
};

error_chain! {
    foreign_links {
        ConnRecv(UnboundedRecvError);
        ConnSend(UnboundedTrySendError<Message>);
        ConnForward(UnboundedSendError);
        Io(::std::io::Error);
        Message(message::errors::Error);
        SubscriberError(RecvError);
        Terminate(::tokio::sync::oneshot::error::RecvError);
    }
}
