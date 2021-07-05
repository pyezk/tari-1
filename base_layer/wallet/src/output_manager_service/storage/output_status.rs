// Copyright 2021. The Tari Project
//
// Redistribution and use in source and binary forms, with or without modification, are permitted provided that the
// following conditions are met:
//
// 1. Redistributions of source code must retain the above copyright notice, this list of conditions and the following
// disclaimer.
//
// 2. Redistributions in binary form must reproduce the above copyright notice, this list of conditions and the
// following disclaimer in the documentation and/or other materials provided with the distribution.
//
// 3. Neither the name of the copyright holder nor the names of its contributors may be used to endorse or promote
// products derived from this software without specific prior written permission.
//
// THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS" AND ANY EXPRESS OR IMPLIED WARRANTIES,
// INCLUDING, BUT NOT LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
// DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL,
// SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR
// SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY,
// WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE
// USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
use core::convert::TryFrom;
use core::result::Result;
use core::result::Result::{Err, Ok};
use crate::output_manager_service::error::OutputManagerStorageError;
use std::fmt;
use std::fmt::Formatter;

/// The status of a given output
#[derive(Debug, Clone, PartialEq)]
pub enum OutputStatus {
    Unspent,
    Spent,
    EncumberedToBeReceived,
    EncumberedToBeSpent,
    Invalid,
    CancelledInbound,
    NotStored
}

impl TryFrom<i32> for OutputStatus {
    type Error = OutputManagerStorageError;

    fn try_from(value: i32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(OutputStatus::Unspent),
            1 => Ok(OutputStatus::Spent),
            2 => Ok(OutputStatus::EncumberedToBeReceived),
            3 => Ok(OutputStatus::EncumberedToBeSpent),
            4 => Ok(OutputStatus::Invalid),
            5 => Ok(OutputStatus::CancelledInbound),
            6 => Ok(OutputStatus::NotStored),
            _ => Err(OutputManagerStorageError::ConversionError),
        }
    }
}

impl fmt::Display for OutputStatus {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            OutputStatus::Unspent => {write!(f, "Unspent")}
            OutputStatus::Spent => {write!(f, "Spent")}
            OutputStatus::EncumberedToBeReceived => {write!(f, "EncumberedToBeReceived")}
            OutputStatus::EncumberedToBeSpent => {write!(f, "EncumberedToBeSpent")}
            OutputStatus::Invalid => {write!(f, "Invalid")}
            OutputStatus::CancelledInbound => {write!(f, "CancelledInbound")}
            OutputStatus::NotStored => {write!(f, "NotStored")}
        }
    }
}