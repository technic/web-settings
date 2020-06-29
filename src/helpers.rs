use futures::prelude::*;
use gotham::handler::{
    Handler, HandlerError, HandlerFuture, IntoHandlerError, IntoHandlerFuture, IntoResponse,
};
use gotham::hyper::{Body, Response};
use gotham::state::{FromState, State};
use std::pin::Pin;

type SimpleResult = Result<Response<Body>, HandlerError>;
type HandlerResult = Result<(State, Response<Body>), (State, HandlerError)>;

pub fn to_handler_result<R>(state: State, result: Result<R, HandlerError>) -> HandlerResult
where
    R: IntoResponse,
{
    match result {
        Ok(r) => {
            let response = r.into_response(&state);
            Ok((state, response))
        }
        Err(e) => Err((state, e)),
    }
}

// Type aliases to help abstract over different mutability inside macro
type Ref<'a, T> = &'a T;
type MutRef<'a, T> = &'a mut T;

// handler implementation for fn that can borrow State by mut or non-mut reference
macro_rules! impl_sync {
    ($handler:ident, $aref:ident) => {
        // Using newtype pattern to implement foreign trait on generic type

        /// Handler that borrows state
        #[derive(Copy, Clone)]
        pub struct $handler<F: Copy>(pub F);

        impl<R, F> Handler for $handler<F>
        where
            F: FnOnce($aref<State>) -> R + Send + Copy,
            R: IntoResponse,
        {
            // We have always mutable here, because non-mutable closure also accepts it
            fn handle(self, mut state: State) -> Pin<Box<HandlerFuture>> {
                let response = (self.0)(&mut state).into_response(&state);
                (state, response).into_handler_future()
            }
        }
    };
}

impl_sync!(SimpleHandler, Ref);
impl_sync!(SimpleMutHandler, MutRef);

// handler implementation for async fn that can borrow State by mut or non-mut reference
macro_rules! impl_async {
    ($handler:ident, $helper:ident, $aref:ident) => {
        /// Wrapper around closure to bind lifetime of Output to lifetime of borrowed argument
        pub trait $helper<'a> {
            type Output: Send + 'a + Future<Output = SimpleResult>;
            fn call(self, arg: $aref<'a, State>) -> Self::Output;
        }

        impl<'a, Fut, Func> $helper<'a> for Func
        where
            Fut: Send + 'a + Future<Output = SimpleResult>,
            Func: FnOnce($aref<'a, State>) -> Fut,
        {
            type Output = Fut;
            fn call(self, arg: $aref<'a, State>) -> Self::Output {
                self(arg)
            }
        }

        // Using newtype pattern to implement foreign trait on generic type
        #[derive(Copy, Clone)]
        pub struct $handler<F>(pub F)
        where
            for<'t> F: $helper<'t> + Send + Copy + 'static;

        impl<F> Handler for $handler<F>
        where
            for<'t> F: $helper<'t> + Copy + Send + Sync + 'static,
        {
            // We have always mutable here, because non-mutable closure also accepts it
            fn handle(self, mut state: State) -> Pin<Box<HandlerFuture>> {
                async move {
                    let fut = self.0.call(&mut state);
                    let result = fut.await;
                    to_handler_result(state, result)
                }
                .boxed()
            }
        }
    };
}

impl_async!(SimpleAsyncHandler, FnHelper, Ref);
impl_async!(SimpleAsyncMutHandler, FnMutHelper, MutRef);
