use alloc::{
    ffi::CString,
    format,
    string::{String, ToString},
};
use core::fmt::{Display, Formatter};

use crate::{Args, Context, Field, LocalObject, Method, Signature, StrongRef, Throwable, Type};

#[cfg(feature = "cache")]
mod cache {
    use alloc::boxed::Box;
    use core::{
        ptr::null_mut,
        sync::atomic::{AtomicPtr, Ordering},
    };

    use crate::{Context, LocalObject, StrongRef, Throwable, Weak};

    const MAX_MEMBER_CACHE_PER_SLOT: usize = 128;

    struct Entry {
        class: Weak,
        types_id: usize,
        name: &'static str,
        member: *const (),
    }

    struct Slot {
        entries: uluru::LRUCache<Entry, MAX_MEMBER_CACHE_PER_SLOT>,
        next: *mut Slot,
    }

    static SLOTS: AtomicPtr<Slot> = AtomicPtr::new(null_mut());

    fn get_or_alloc_slot() -> &'static mut Slot {
        unsafe {
            loop {
                match SLOTS.load(Ordering::Relaxed).as_mut() {
                    None => {
                        break Box::leak(Box::new(Slot {
                            entries: uluru::LRUCache::new(),
                            next: null_mut(),
                        }));
                    }
                    Some(current) => match SLOTS.compare_exchange(current, current.next, Ordering::Relaxed, Ordering::Relaxed) {
                        Ok(_) => {
                            current.next = null_mut();

                            break current;
                        }
                        Err(_) => continue,
                    },
                }
            }
        }
    }

    fn put_slot(slot: &'static mut Slot) {
        loop {
            let next = SLOTS.load(Ordering::Relaxed);

            slot.next = next;

            match SLOTS.compare_exchange(next, slot, Ordering::Relaxed, Ordering::Relaxed) {
                Ok(_) => break,
                Err(_) => continue,
            }
        }
    }

    fn use_a_slot<R, F>(f: F) -> R
    where
        for<'a> F: FnOnce(&'a mut &'static mut Slot) -> R,
    {
        let mut slot = get_or_alloc_slot();

        let r = f(&mut slot);

        put_slot(slot);

        r
    }

    pub fn find_member<
        'ctx,
        C: StrongRef,
        M: Copy,
        F: FnOnce(Option<*const ()>) -> Result<(M, *const ()), LocalObject<'ctx, Throwable>>,
    >(
        ctx: &'ctx Context,
        class: &C,
        name: &'static str,
        find: F,
    ) -> Result<M, LocalObject<'ctx, Throwable>> {
        use_a_slot(|slot| {
            let types_id = find_member::<C, M, F> as *const () as usize;

            let cached = slot.entries.find(|e| {
                e.types_id == types_id && name.as_ptr() == e.name.as_ptr() && ctx.is_same_object(Some(&e.class), Some(class))
            });
            match cached {
                Some(e) => Ok(find(Some(e.member))?.0),
                None => {
                    let (member, cache) = find(None)?;

                    slot.entries.insert(Entry {
                        class: class.downgrade_weak(),
                        types_id,
                        name,
                        member: cache,
                    });

                    Ok(member)
                }
            }
        })
    }
}

pub fn method_signature_of(args: &[Signature], ret: &Signature) -> String {
    struct ArgsSignature<'a>(&'a [Signature]);

    impl<'a> Display for ArgsSignature<'a> {
        fn fmt(&self, f: &mut Formatter<'_>) -> alloc::fmt::Result {
            for x in self.0.iter() {
                x.fmt(f)?
            }

            Ok(())
        }
    }

    format!("({}){}", ArgsSignature(args), ret)
}

pub fn find_method<'a, 'ctx, const STATIC: bool, C: StrongRef, A: Args<'a>, R: Type>(
    ctx: &'ctx Context,
    class: &C,
    name: &'static str,
) -> Result<Method<STATIC>, LocalObject<'ctx, Throwable>>
where
    A::Array<Signature>: AsRef<[Signature]>,
{
    #[cfg(feature = "cache")]
    return {
        use crate::{FromRaw, IntoRaw};

        cache::find_member(ctx, class, name, |cached| match cached {
            Some(ptr) => unsafe { Ok((Method::from_raw(ptr as _), ptr)) },
            None => {
                let m = ctx.find_method(
                    class,
                    CString::new(name).unwrap(),
                    CString::new(method_signature_of(A::signatures().as_ref(), &R::SIGNATURE)).unwrap(),
                )?;

                Ok((m, m.into_raw() as *const ()))
            }
        })
    };

    #[cfg(not(feature = "cache"))]
    ctx.find_method(
        class,
        CString::new(name).unwrap(),
        CString::new(method_signature_of(A::signatures().as_ref(), &R::SIGNATURE)).unwrap(),
    )
}

pub fn find_field<'a, 'ctx, const STATIC: bool, C: StrongRef, T: Type>(
    ctx: &'ctx Context,
    class: &C,
    name: &'static str,
) -> Result<Field<STATIC>, LocalObject<'ctx, Throwable>> {
    #[cfg(feature = "cache")]
    return {
        use crate::{FromRaw, IntoRaw};

        cache::find_member(ctx, class, name, |cached| match cached {
            Some(ptr) => unsafe { Ok((Field::from_raw(ptr as _), ptr)) },
            None => {
                let f = ctx.find_field(
                    class,
                    CString::new(name).unwrap(),
                    CString::new(T::SIGNATURE.to_string()).unwrap(),
                )?;

                Ok((f, f.into_raw() as _))
            }
        })
    };

    #[cfg(not(feature = "cache"))]
    ctx.find_field(
        class,
        CString::new(name).unwrap(),
        CString::new(T::SIGNATURE.to_string()).unwrap(),
    )
}
