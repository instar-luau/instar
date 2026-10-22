mod dispatch;
mod message;
mod native;
mod operation;
mod source;

pub(crate) use self::{
    dispatch::Connection,
    message::Target,
    operation::{Consistency, Operation},
};
