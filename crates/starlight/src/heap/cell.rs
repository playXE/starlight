use std::{
    any::type_name,
    collections::HashMap,
    marker::PhantomData,
    mem::size_of,
    ops::{Deref, DerefMut},
    ptr::NonNull,
    sync::atomic::{AtomicU8, Ordering},
};

use super::{
    snapshot::deserializer::Deserializable, snapshot::serializer::Serializable, SlotVisitor,
};
use mopa::mopafy;

/// Indicates that a type can be traced by a garbage collector.
///
/// This doesn't necessarily mean that the type is safe to allocate in a garbage collector ([GcCell]).
///
/// ## Safety
/// See the documentation of the `trace` method for more info.
/// Essentially, this object must faithfully trace anything that
/// could contain garbage collected pointers or other `Trace` items.
pub unsafe trait Trace {
    /// Visit each field in this type
    ///
    ///
    /// Structures should trace each of their fields,
    /// and collections should trace each of their elements.
    ///
    /// ### Safety
    /// Some types (like `GcPointer`) need special actions taken when they're traced,
    /// but those are somewhat rare and are usually already provided by the garbage collector.
    ///
    /// ## Always Permitted
    /// - Reading your own memory (includes iteration)
    ///   - Interior mutation is undefined behavior, even if you use `RefCell`
    /// - Panicking
    ///   - This should be reserved for cases where you are seriously screwed up,
    ///       and can't fulfill your contract to trace your interior properly.
    ///   - This rule may change in future versions, depending on how we deal with multi-threading.
    /// ## Never Permitted Behavior
    /// - Forgetting a element of a collection, or field of a structure
    ///   - If you forget an element undefined behavior will result
    ///   - This is why we always prefer automatically derived implementations where possible.
    ///     - You will never trigger undefined behavior with an automatic implementation,
    ///       and it'll always be completely sufficient for safe code (aside from destructors).
    ///     - With an automatically derived implementation you will never miss a field
    /// - Invoking this function directly.
    fn trace(&self, visitor: &mut SlotVisitor) {
        let _ = visitor;
    }
}

/// `GcCell` is a type that can be allocated in GC heap and passed to JavaScript environment.
///
///
/// All cells that is not part of `src/vm` treatened as dummy objects and property accesses
/// is no-op on them.
///
pub trait GcCell: mopa::Any + Trace + Serializable {
    /// Used when object has dynamic size i.e arrays
    fn compute_size(&self) -> usize {
        std::mem::size_of_val(self)
    }

    fn type_name(&self) -> &'static str {
        std::any::type_name::<Self>()
    }

    fn deser_pair(&self) -> (usize, usize);
}

mopafy!(GcCell);

#[repr(C)]
pub struct GcPointerBase {
    vtable: u64,
    cell_state: AtomicU8, //pub next: *mut Self,
}

pub const POSSIBLY_BLACK: u8 = 0;
pub const POSSIBLY_GREY: u8 = 2;
pub const DEFINETELY_WHITE: u8 = 1;

impl GcPointerBase {
    pub fn new(vtable: usize) -> Self {
        Self {
            vtable: (vtable as u64 | 0),
            cell_state: AtomicU8::new(DEFINETELY_WHITE),
            //  next: null_mut(),
            //mark: false,
            // dead: true,
        }
    }

    pub fn test_and_set_marked(&mut self) -> bool {
        let prev = self.vtable & (1 << 0);
        self.vtable |= 1 << 0;
        prev == 0
    }

    pub fn unmark(&mut self) {
        self.vtable &= !(1 << 0);
    }

    pub fn state(&self) -> u8 {
        self.cell_state.load(Ordering::Acquire)
    }

    pub fn set_state(&self, from: u8, to: u8) -> bool {
        self.cell_state
            .compare_exchange(from, to, Ordering::AcqRel, Ordering::Relaxed)
            == Ok(from)
    }
    pub fn force_set_state(&self, to: u8) {
        self.cell_state.store(to, Ordering::AcqRel);
    }
    pub fn data<T>(&self) -> *mut T {
        unsafe {
            (self as *const Self as *mut u8)
                .add(size_of::<Self>())
                .cast()
        }
    }
    pub fn raw(&self) -> u64 {
        self.vtable
    }

    pub fn get_dyn(&self) -> &mut dyn GcCell {
        unsafe {
            std::mem::transmute(mopa::TraitObject {
                vtable: (self.vtable & !(1 << 0)) as *mut (),
                data: self.data::<u8>() as _,
            })
        }
    }

    pub fn vtable(&self) -> usize {
        (self.vtable & !(1 << 0)) as usize
    }

    pub fn is_marked(&self) -> bool {
        (self.vtable & (1 << 0)) != 0
    }
}
pub fn vtable_of<T: GcCell>(x: *const T) -> usize {
    unsafe { core::mem::transmute::<_, mopa::TraitObject>(x as *const dyn GcCell).vtable as _ }
}

pub fn vtable_of_type<T: GcCell + Sized>() -> usize {
    vtable_of(core::ptr::null::<T>())
}

/// A garbage collected pointer to a value.
///
/// This is the equivalent of a garbage collected smart-pointer.
///
///
/// The smart pointer is simply a guarantee to the garbage collector
/// that this points to a garbage collected object with the correct header,
/// and not some arbitrary bits that you've decided to heap allocate.
pub struct GcPointer<T: ?Sized> {
    pub(crate) base: NonNull<GcPointerBase>,
    pub(super) marker: PhantomData<T>,
}

impl<T: GcCell + ?Sized> GcPointer<T> {
    pub fn ptr_eq<U: GcCell + ?Sized>(this: &Self, other: &GcPointer<U>) -> bool {
        this.base == other.base
    }
    #[inline]
    pub fn as_dyn(self) -> GcPointer<dyn GcCell> {
        GcPointer {
            base: self.base,
            marker: PhantomData,
        }
    }
}

impl<T: GcCell + ?Sized> GcPointer<T> {
    #[inline]
    pub fn is<U: GcCell>(self) -> bool {
        unsafe { (*self.base.as_ptr()).vtable() == vtable_of_type::<U>() }
    }

    #[inline]
    pub fn get_dyn(&self) -> &dyn GcCell {
        unsafe { (*self.base.as_ptr()).get_dyn() }
    }

    #[inline]
    pub fn get_dyn_mut(&mut self) -> &mut dyn GcCell {
        unsafe { (*self.base.as_ptr()).get_dyn() }
    }

    #[inline]
    pub unsafe fn downcast_unchecked<U: GcCell>(self) -> GcPointer<U> {
        GcPointer {
            base: self.base,
            marker: PhantomData,
        }
    }

    #[inline]
    pub fn downcast<U: GcCell>(self) -> Option<GcPointer<U>> {
        if !self.is::<U>() {
            None
        } else {
            Some(unsafe { self.downcast_unchecked() })
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum WeakState {
    Free = 0,
    Unmarked,
    Mark,
}
pub struct WeakSlot {
    pub(super) state: WeakState,
    pub(super) value: *mut GcPointerBase,
}

pub struct WeakRef<T: GcCell> {
    pub(crate) inner: NonNull<WeakSlot>,
    pub(super) marker: PhantomData<T>,
}

impl<T: GcCell> WeakRef<T> {
    pub fn upgrade(&self) -> Option<GcPointer<T>> {
        unsafe {
            let inner = &*self.inner.as_ptr();
            if inner.value.is_null() {
                println!("upgrade failed");
                return None;
            }

            Some(GcPointer {
                base: NonNull::new_unchecked(inner.value),
                marker: PhantomData::<T>,
            })
        }
    }
}

macro_rules! impl_prim {
    ($($t: ty)*) => {
        $(
            unsafe impl Trace for $t {}
            impl GcCell for $t {
                fn deser_pair(&self) -> (usize,usize) {
                    (Self::deserialize as usize,Self::allocate as usize)
                }
            }
        )*
    };
}

impl_prim!(String bool f32 f64 u8 i8 u16 i16 u32 i32 u64 i64 );
unsafe impl<T: Trace> Trace for Vec<T> {
    fn trace(&self, visitor: &mut SlotVisitor) {
        for val in self.iter() {
            val.trace(visitor);
        }
    }
}

unsafe impl<T: GcCell + ?Sized> Trace for GcPointer<T> {
    fn trace(&self, visitor: &mut SlotVisitor) {
        visitor.visit(self);
    }
}

impl<T: GcCell + ?Sized> Copy for GcPointer<T> {}
impl<T: GcCell + ?Sized> Clone for GcPointer<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: GcCell> Deref for GcPointer<T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        unsafe { &*(&*self.base.as_ptr()).data::<T>() }
    }
}
impl<T: GcCell> DerefMut for GcPointer<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { &mut *(&*self.base.as_ptr()).data::<T>() }
    }
}

impl<T: GcCell> std::fmt::Pointer for GcPointer<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:p}", self.base)
    }
}

impl<T: GcCell + std::fmt::Debug> std::fmt::Debug for GcPointer<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", **self)
    }
}
impl<T: GcCell + std::fmt::Display> std::fmt::Display for GcPointer<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", **self)
    }
}

impl<T: GcCell> GcCell for WeakRef<T> {
    fn deser_pair(&self) -> (usize, usize) {
        (Self::deserialize as _, Self::allocate as _)
    }
}
unsafe impl<T: GcCell> Trace for WeakRef<T> {
    fn trace(&self, visitor: &mut SlotVisitor) {
        visitor.visit_weak(self);
    }
}

unsafe impl<K: Trace, V: Trace> Trace for HashMap<K, V> {
    fn trace(&self, visitor: &mut SlotVisitor) {
        for (key, value) in self.iter() {
            key.trace(visitor);
            value.trace(visitor);
        }
    }
}

impl<
        K: GcCell + Eq + std::hash::Hash + Trace + 'static + Serializable + Deserializable,
        V: GcCell + Trace + 'static + Serializable + Deserializable,
    > GcCell for HashMap<K, V>
{
    fn deser_pair(&self) -> (usize, usize) {
        (Self::deserialize as _, Self::allocate as _)
    }
}

unsafe impl<T: Trace> Trace for Option<T> {
    fn trace(&self, visitor: &mut SlotVisitor) {
        match self {
            Some(val) => val.trace(visitor),
            _ => (),
        }
    }
}

impl<T: GcCell + Serializable + 'static + Deserializable> GcCell for Vec<T> {
    fn deser_pair(&self) -> (usize, usize) {
        (Self::deserialize as usize, Self::allocate as usize)
    }
}
impl<T: GcCell + ?Sized> GcCell for GcPointer<T> {
    fn deser_pair(&self) -> (usize, usize) {
        (Self::deserialize as _, Self::allocate as _)
    }
}

impl<T: GcCell + Serializable + Deserializable + 'static> GcCell for Option<T> {
    fn deser_pair(&self) -> (usize, usize) {
        (Self::deserialize as _, Self::allocate as _)
    }
}
