/*
 * Copyright 2022 Google Inc. All Rights Reserved.
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *     http://www.apache.org/licenses/LICENSE-2.0
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

#![doc = include_str!("../README.md")]

// Note: `atomics` is whitelisted in `target_feature` detection, but `bulk-memory` isn't,
// so we can check only presence of the former. This should be enough to catch most common
// mistake (forgetting to pass `RUSTFLAGS` altogether).
#[cfg(all(target_arch = "wasm32", not(doc), not(target_feature = "atomics")))]
compile_error!("Did you forget to enable `atomics` and `bulk-memory` features as outlined in wasm-bindgen-rayon README?");

use crossbeam_channel::{bounded, Receiver, Sender};
use js_sys::Promise;
use rayon::{ThreadBuilder, ThreadPoolBuilder};
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsValue;

#[cfg(feature = "no-bundler")]
use js_sys::JsString;

// Naming is a workaround for https://github.com/rustwasm/wasm-bindgen/issues/2429
// and https://github.com/rustwasm/wasm-bindgen/issues/1762.
#[allow(non_camel_case_types)]
#[wasm_bindgen]
#[doc(hidden)]
pub struct wbg_rayon_PoolBuilder {
    num_threads: usize,
    sender: Sender<ThreadBuilder>,
    receiver: Receiver<ThreadBuilder>,
}

#[cfg_attr(
    not(feature = "no-bundler"),
    wasm_bindgen(module = "/src/workerHelpers.js")
)]
#[cfg_attr(
    feature = "no-bundler",
    wasm_bindgen(module = "/src/workerHelpers.no-bundler.js")
)]
extern "C" {
    #[wasm_bindgen(js_name = startWorkers)]
    fn start_workers(module: JsValue, memory: JsValue, builder: wbg_rayon_PoolBuilder) -> Promise;
}

#[wasm_bindgen]
impl wbg_rayon_PoolBuilder {
    fn new(num_threads: usize) -> Self {
        #[cfg(debug_assertions)]
        if num_threads == 0 {
            wasm_bindgen::throw_str("Number of threads must be greater than zero.");
        }
        let (sender, receiver) = bounded(num_threads);
        Self {
            num_threads,
            sender,
            receiver,
        }
    }

    #[cfg(feature = "no-bundler")]
    #[wasm_bindgen(js_name = mainJS)]
    pub fn main_js(&self) -> JsString {
        #[wasm_bindgen]
        extern "C" {
            #[wasm_bindgen(thread_local_v2, js_namespace = ["import", "meta"], js_name = url)]
            static URL: JsString;
        }

        URL.with(Clone::clone)
    }

    #[wasm_bindgen(js_name = numThreads)]
    pub fn num_threads(&self) -> usize {
        self.num_threads
    }

    pub fn receiver(&self) -> *const Receiver<ThreadBuilder> {
        &self.receiver
    }

    // This should be called by the JS side once all the Workers are spawned.
    // Important: it must take `self` by reference, otherwise
    // `start_worker_thread` will try to receive a message on a moved value.
    pub fn build(&mut self) {
        ThreadPoolBuilder::new()
            .num_threads(self.num_threads)
            // We could use postMessage here instead of Rust channels,
            // but currently we can't due to a Chrome bug that will cause
            // the main thread to lock up before it even sends the message:
            // https://bugs.chromium.org/p/chromium/issues/detail?id=1075645
            .spawn_handler(move |thread| {
                // Note: `send` will return an error if there are no receivers.
                // We can use it because all the threads are spawned and ready to accept
                // messages by the time we call `build()` to instantiate spawn handler.
                self.sender.send(thread).unwrap_throw();
                Ok(())
            })
            .build_global()
            .unwrap_throw();
    }
}

/// Function exposed as `initThreadPool` to JS (see the main docs).
///
/// Normally, you'd invoke this function from JS to initialize the thread pool.
/// However, if you strongly prefer, you can use [wasm-bindgen-futures](https://rustwasm.github.io/wasm-bindgen/reference/js-promises-and-rust-futures.html) to invoke and await this function from Rust.
///
/// Note that doing so comes with extra initialization and Wasm size overhead for the JS<->Rust Promise integration.
#[wasm_bindgen(js_name = initThreadPool)]
pub fn init_thread_pool(num_threads: usize) -> Promise {
    start_workers(
        wasm_bindgen::module(),
        wasm_bindgen::memory(),
        wbg_rayon_PoolBuilder::new(num_threads),
    )
}

#[wasm_bindgen]
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[doc(hidden)]
pub fn wbg_rayon_start_worker(receiver: *const Receiver<ThreadBuilder>)
where
    // Statically assert that it's safe to accept `Receiver` from another thread.
    Receiver<ThreadBuilder>: Sync,
{
    // This is safe, because we know it came from a reference to PoolBuilder,
    // allocated on the heap by wasm-bindgen and dropped only once all the
    // threads are running.
    //
    // The only way to violate safety is if someone externally calls
    // `exports.wbg_rayon_start_worker(garbageValue)`, but then no Rust tools
    // would prevent us from issues anyway.
    let receiver = unsafe { &*receiver };
    // Wait for a task (`ThreadBuilder`) on the channel, and, once received,
    // start executing it.
    //
    // On practice this will start running Rayon's internal event loop.
    receiver.recv().unwrap_throw().run()
}
