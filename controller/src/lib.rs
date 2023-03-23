// Copyright 2019 The Epic Developers
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Library module for the main wallet functionalities provided by Epic.

#[macro_use]
extern crate prettytable;

#[macro_use]
extern crate log;
#[macro_use]
extern crate lazy_static;
use stack_epic_wallet_api as apiwallet;
use stack_epic_wallet_config as config;
use stack_epic_wallet_impls as impls;
use stack_epic_wallet_libwallet as libwallet;
use stack_epic_wallet_util::stack_epic_api as api;
use stack_epic_wallet_util::stack_epic_core as core;
use stack_epic_wallet_util::stack_epic_keychain as keychain;
use stack_epic_wallet_util::stack_epic_util as util;
use failure;

pub mod command;
pub mod controller;
pub mod display;
mod error;

pub use crate::error::{Error, ErrorKind};
