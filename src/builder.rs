use crate::span::Span;
pub trait Builder<'s> {
    type Out: 's;
    /// Moves the builder into its built form. A list keeps its memory.
    fn build(self, file: &'s str) -> Self::Out;
}
#[macro_export]
macro_rules! self_builder {
    ($t:ty) => {
        impl<'s> $crate::builder::Builder<'s> for $t {
            type Out = $t;
            #[inline]
            fn build(self, _file: &'s str) -> Self::Out {
                self
            }
        }
    };
}
self_builder!(usize);
self_builder!(f64);
self_builder!(u8);

impl<'s> Builder<'s> for Span {
    type Out = &'s str;
    #[inline]
    fn build(self, file: &'s str) -> Self::Out {
        &file[&self]
    }
}

impl<'s, T: Builder<'s>> Builder<'s> for Vec<T> {
    type Out = Vec<T::Out>;
    #[inline]
    fn build(self, file: &'s str) -> Self::Out {
        self.into_iter().map(|s| s.build(file)).collect()
    }
}

impl<'s, T: Builder<'s>> Builder<'s> for Box<T> {
    type Out = Box<T::Out>;
    #[inline]
    fn build(self, file: &'s str) -> Self::Out {
        Box::new((*self).build(file))
    }
}

impl<'s, T: Builder<'s>> Builder<'s> for Option<T> {
    type Out = Option<T::Out>;
    #[inline]
    fn build(self, file: &'s str) -> Self::Out {
        self.map(|s| s.build(file))
    }
}
impl<'s, T1: Builder<'s>, T2: Builder<'s>> Builder<'s> for (T1, T2) {
    type Out = (T1::Out, T2::Out);
    #[inline]
    fn build(self, file: &'s str) -> Self::Out {
        (self.0.build(file), self.1.build(file))
    }
}
impl<'s, T1: Builder<'s>, T2: Builder<'s>, T3: Builder<'s>> Builder<'s> for (T1, T2, T3) {
    type Out = (T1::Out, T2::Out, T3::Out);
    #[inline]
    fn build(self, file: &'s str) -> Self::Out {
        (self.0.build(file), self.1.build(file), self.2.build(file))
    }
}
