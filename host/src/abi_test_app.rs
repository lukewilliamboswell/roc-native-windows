//! Host unit tests link without a Roc application. Reaching an entry point
//! means a test must use a real app scenario instead.

use crate::roc_platform_abi::{
    AnonStructB5d81caf03d08956, AnonStructF29a1b847614096c, CoreKept, RocBox, RocList, RocListWith,
};

type Bytes = RocListWith<u8, false>;
type Files = RocList<*mut u64>;
type Step = AnonStructF29a1b847614096c;
type Answer = AnonStructB5d81caf03d08956;

macro_rules! absent_app {
    ($(fn $name:ident($($arg:ident: $kind:ty),*) $(-> $out:ty)?;)*) => {
        $(
            #[unsafe(no_mangle)]
            extern "C" fn $name($($arg: $kind),*) $(-> $out)? {
                panic!("host unit test reached {}", stringify!($name));
            }
        )*
    };
}

absent_app! {
    fn roc_engine_ask(_state: *mut CoreKept, _task: Bytes, _files: Files) -> Answer;
    fn roc_engine_drop(_state: RocBox, _buffers: Bytes, _work: Bytes);
    fn roc_engine_event(_state: RocBox, _buffers: Bytes, _work: Bytes, _node: u32, _handler: u32, _payload: Bytes) -> Step;
    fn roc_engine_files(_state: RocBox, _buffers: Bytes, _work: Bytes, _node: u32, _handler: u32, _payload: Bytes, _files: Files) -> Step;
    fn roc_engine_forget(_state: *mut CoreKept);
    fn roc_engine_init(_boot: Bytes) -> Step;
    fn roc_engine_keys() -> Bytes;
    fn roc_engine_prepare(_task: Bytes, _files: Files) -> *mut CoreKept;
    fn roc_engine_version() -> u32;
    fn roc_engine_work(_task: Bytes, _files: Files) -> Bytes;
}
