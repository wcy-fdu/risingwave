// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

#[cfg(not(feature = "std"))]
use alloc::boxed::Box;
use core::fmt;

use risingwave_common::error::{ErrorCode, Result};
use risingwave_common::types::DataType as Common_Data_Type;
#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

use crate::ast::ObjectName;

/// SQL data types
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum DataType {
    /// Fixed-length character type e.g. CHAR(10)
    Char(Option<u64>),
    /// Variable-length character type e.g. VARCHAR(10)
    Varchar(Option<u64>),
    /// Uuid type
    Uuid,
    /// Large character object e.g. CLOB(1000)
    Clob(u64),
    /// Fixed-length binary type e.g. BINARY(10)
    Binary(u64),
    /// Variable-length binary type e.g. VARBINARY(10)
    Varbinary(u64),
    /// Large binary object e.g. BLOB(1000)
    Blob(u64),
    /// Decimal type with optional precision and scale e.g. DECIMAL(10,2)
    Decimal(Option<u64>, Option<u64>),
    /// Floating point with optional precision e.g. FLOAT(8)
    Float(Option<u64>),
    /// Tiny integer with optional display width e.g. TINYINT or TINYINT(3)
    TinyInt(Option<u64>),
    /// Small integer with optional display width e.g. SMALLINT or SMALLINT(5)
    SmallInt(Option<u64>),
    /// Integer with optional display width e.g. INT or INT(11)
    Int(Option<u64>),
    /// Big integer with optional display width e.g. BIGINT or BIGINT(20)
    BigInt(Option<u64>),
    /// Floating point e.g. REAL
    Real,
    /// Double e.g. DOUBLE PRECISION
    Double,
    /// Boolean
    Boolean,
    /// Date
    Date,
    /// Time with optional time zone
    Time(bool),
    /// Timestamp with optional time zone
    Timestamp(bool),
    /// Interval
    Interval,
    /// Regclass used in postgresql serial
    Regclass,
    /// Text
    Text,
    /// String
    String,
    /// Bytea
    Bytea,
    /// Custom type such as enums
    Custom(ObjectName),
    /// Arrays
    Array(Box<DataType>),
    Struct,
}

impl DataType {
    pub fn to_data_type(&self) -> Result<Common_Data_Type> {
        let data_type = match self {
            DataType::Boolean => Common_Data_Type::Boolean,
            DataType::SmallInt(None) => Common_Data_Type::Int16,
            DataType::Int(None) => Common_Data_Type::Int32,
            DataType::BigInt(None) => Common_Data_Type::Int64,
            DataType::Real | DataType::Float(Some(1..=24)) => Common_Data_Type::Float32,
            DataType::Double | DataType::Float(Some(25..=53) | None) => Common_Data_Type::Float64,
            DataType::Decimal(None, None) => Common_Data_Type::Decimal,
            DataType::Varchar(_) => Common_Data_Type::Varchar,
            DataType::Date => Common_Data_Type::Date,
            DataType::Time(false) => Common_Data_Type::Time,
            DataType::Timestamp(false) => Common_Data_Type::Timestamp,
            DataType::Timestamp(true) => Common_Data_Type::Timestampz,
            DataType::Interval => Common_Data_Type::Interval,
            DataType::Array(datatype) => Common_Data_Type::List {
                datatype: Box::new(datatype.to_data_type()?),
            },
            DataType::Char(..) => {
                return Err(ErrorCode::NotImplemented(
                    "CHAR is not supported, please use VARCHAR instead\n".to_string(),
                    None.into(),
                )
                .into())
            }
            _ => {
                return Err(ErrorCode::NotImplemented(
                    format!("unsupported data type: {:?}", self),
                    None.into(),
                )
                .into())
            }
        };
        Ok(data_type)
    }
}

impl fmt::Display for DataType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            DataType::Char(size) => format_type_with_optional_length(f, "CHAR", size),
            DataType::Varchar(size) => {
                format_type_with_optional_length(f, "CHARACTER VARYING", size)
            }
            DataType::Uuid => write!(f, "UUID"),
            DataType::Clob(size) => write!(f, "CLOB({})", size),
            DataType::Binary(size) => write!(f, "BINARY({})", size),
            DataType::Varbinary(size) => write!(f, "VARBINARY({})", size),
            DataType::Blob(size) => write!(f, "BLOB({})", size),
            DataType::Decimal(precision, scale) => {
                if let Some(scale) = scale {
                    write!(f, "NUMERIC({},{})", precision.unwrap(), scale)
                } else {
                    format_type_with_optional_length(f, "NUMERIC", precision)
                }
            }
            DataType::Float(size) => format_type_with_optional_length(f, "FLOAT", size),
            DataType::TinyInt(zerofill) => format_type_with_optional_length(f, "TINYINT", zerofill),
            DataType::SmallInt(zerofill) => {
                format_type_with_optional_length(f, "SMALLINT", zerofill)
            }
            DataType::Int(zerofill) => format_type_with_optional_length(f, "INT", zerofill),
            DataType::BigInt(zerofill) => format_type_with_optional_length(f, "BIGINT", zerofill),
            DataType::Real => write!(f, "REAL"),
            DataType::Double => write!(f, "DOUBLE"),
            DataType::Boolean => write!(f, "BOOLEAN"),
            DataType::Date => write!(f, "DATE"),
            DataType::Time(tz) => write!(f, "TIME{}", if *tz { " WITH TIME ZONE" } else { "" }),
            DataType::Timestamp(tz) => {
                write!(f, "TIMESTAMP{}", if *tz { " WITH TIME ZONE" } else { "" })
            }
            DataType::Interval => write!(f, "INTERVAL"),
            DataType::Regclass => write!(f, "REGCLASS"),
            DataType::Text => write!(f, "TEXT"),
            DataType::String => write!(f, "STRING"),
            DataType::Bytea => write!(f, "BYTEA"),
            DataType::Array(ty) => write!(f, "{}[]", ty),
            DataType::Custom(ty) => write!(f, "{}", ty),
            DataType::Struct => write!(f, "STRUCT"),
        }
    }
}

fn format_type_with_optional_length(
    f: &mut fmt::Formatter,
    sql_type: &'static str,
    len: &Option<u64>,
) -> fmt::Result {
    write!(f, "{}", sql_type)?;
    if let Some(len) = len {
        write!(f, "({})", len)?;
    }
    Ok(())
}
