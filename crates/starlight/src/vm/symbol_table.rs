use dashmap::DashMap;
use std::sync::atomic::Ordering;
use std::{mem::MaybeUninit, sync::atomic::AtomicU32};

use crate::heap::cell::{GcCell, GcPointer, Trace};

use super::Runtime;
pub struct SymbolTable {
    symbols: DashMap<&'static str, u32>,
    ids: DashMap<u32, &'static str>,
    key: AtomicU32,
}

impl SymbolTable {
    pub fn new() -> Self {
        Self {
            symbols: DashMap::with_capacity(0),
            ids: DashMap::with_capacity(0),
            key: AtomicU32::new(128),
        }
    }

    pub fn description(&self, symbol: SymbolID) -> &'static str {
        *self.ids.get(&symbol.0).unwrap()
    }
    pub fn intern(&self, val: impl AsRef<str>) -> SymbolID {
        let string = val.as_ref();
        if let Some(key) = self.symbols.get(string) {
            return SymbolID(*key.value());
        }

        let string = Box::leak(string.to_string().into_boxed_str());
        let make_new_key = || self.key.fetch_add(1, Ordering::Relaxed);
        let key = *self
            .symbols
            .entry(string)
            .or_insert_with(make_new_key)
            .value();
        self.ids.insert(key, string);
        SymbolID(key)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Hash)]
pub struct SymbolID(u32);

impl SymbolID {
    pub const PUBLIC_START: SymbolID = Self(128);
}
/// Runtime symbol type.
///
///
/// This type is used as property names and inside JsSymbol.
#[derive(Clone, Copy, Eq, PartialEq, Hash)]
pub enum Symbol {
    /// Represents index value, this variant is used when you can definetely put array
    /// index inside u32 so it does not take space in interner heap.
    Key(SymbolID),
    Index(u32),
}

impl GcCell for Symbol {}
unsafe impl Trace for Symbol {}

pub const DUMMY_SYMBOL: Symbol = Symbol::Key(SymbolID(0));

static mut TABLE: MaybeUninit<SymbolTable> = MaybeUninit::uninit();

pub(crate) fn initialize_symbol_table() {
    unsafe {
        TABLE.as_mut_ptr().write(SymbolTable::new());
    }
}
pub fn symbol_table() -> &'static SymbolTable {
    unsafe { &*TABLE.as_ptr() }
}
pub trait Internable {
    fn intern(&self) -> Symbol;
}

impl Internable for str {
    fn intern(&self) -> Symbol {
        Symbol::Key(symbol_table().intern(self))
    }
}

impl Internable for String {
    fn intern(&self) -> Symbol {
        Symbol::Key(symbol_table().intern(self))
    }
}

impl Internable for u32 {
    fn intern(&self) -> Symbol {
        Symbol::Index(*self)
    }
}

impl Internable for usize {
    fn intern(&self) -> Symbol {
        if *self as u32 as usize == *self {
            return (*self as u32).intern();
        }
        self.to_string().intern()
    }
}

pub struct JsSymbol {
    sym: Symbol,
}

impl JsSymbol {
    pub fn new(rt: &mut Runtime, sym: Symbol) -> GcPointer<Self> {
        rt.heap().allocate(Self { sym })
    }

    pub fn symbol(&self) -> Symbol {
        self.sym
    }
}

unsafe impl Trace for JsSymbol {}
impl GcCell for JsSymbol {}