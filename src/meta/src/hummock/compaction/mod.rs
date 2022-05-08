// Copyright 2022 Singularity Data
//
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

mod overlap_strategy;
mod tier_compaction_picker;

use std::io::Cursor;

use itertools::Itertools;
use prost::Message;
use risingwave_common::error::Result;
use risingwave_hummock_sdk::key_range::KeyRange;
use risingwave_hummock_sdk::HummockEpoch;
use risingwave_pb::hummock::{
    CompactMetrics, CompactTask, HummockVersion, Level, TableSetStatistics,
};

use crate::hummock::compaction::overlap_strategy::RangeOverlapStrategy;
use crate::hummock::compaction::tier_compaction_picker::TierCompactionPicker;
use crate::hummock::level_handler::LevelHandler;
use crate::hummock::model::HUMMOCK_DEFAULT_CF_NAME;
use crate::model::Transactional;
use crate::storage;
use crate::storage::{MetaStore, Transaction};

/// Hummock `compact_status` key
/// `cf(hummock_default)`: `hummock_compact_status_key` -> `CompactStatus`
pub(crate) const HUMMOCK_COMPACT_STATUS_KEY: &str = "compact_status";

#[derive(Clone, PartialEq, Debug)]
pub struct CompactStatus {
    pub(crate) level_handlers: Vec<LevelHandler>,
    pub(crate) next_compact_task_id: u64,
}

pub struct SearchResult {
    select_level: Level,
    target_level: Level,
    split_ranges: Vec<KeyRange>,
}

impl CompactStatus {
    pub fn new() -> CompactStatus {
        let vec_handler_having_l0 = vec![LevelHandler::new(0), LevelHandler::new(1)];
        CompactStatus {
            level_handlers: vec_handler_having_l0,
            next_compact_task_id: 1,
        }
    }

    fn cf_name() -> &'static str {
        HUMMOCK_DEFAULT_CF_NAME
    }

    fn key() -> &'static str {
        HUMMOCK_COMPACT_STATUS_KEY
    }

    pub async fn get<S: MetaStore>(meta_store: &S) -> Result<Option<CompactStatus>> {
        match meta_store
            .get_cf(CompactStatus::cf_name(), CompactStatus::key().as_bytes())
            .await
            .map(|v| risingwave_pb::hummock::CompactStatus::decode(&mut Cursor::new(v)).unwrap())
            .map(|s| (&s).into())
        {
            Ok(compact_status) => Ok(Some(compact_status)),
            Err(err) => {
                if !matches!(err, storage::Error::ItemNotFound(_)) {
                    return Err(err.into());
                }
                Ok(None)
            }
        }
    }

    pub fn get_compact_task(&mut self, levels: Vec<Level>) -> Option<CompactTask> {
        // When we compact the files, we must make the result of compaction meet the following
        // conditions, for any user key, the epoch of it in the file existing in the lower
        // layer must be larger.

        let ret = match self.pick_compaction(levels) {
            Some(ret) => ret,
            None => return None,
        };

        let select_level_id = ret.select_level.level_idx;
        let target_level_id = ret.target_level.level_idx;

        let compact_task = CompactTask {
            input_ssts: vec![ret.select_level, ret.target_level],
            splits: ret
                .split_ranges
                .iter()
                .map(|v| v.clone().into())
                .collect_vec(),
            watermark: HummockEpoch::MAX,
            sorted_output_ssts: vec![],
            task_id: self.next_compact_task_id,
            target_level: target_level_id,
            is_target_ultimate_and_leveling: target_level_id as usize
                == self.level_handlers.len() - 1
                && select_level_id > 0,
            metrics: Some(CompactMetrics {
                read_level_n: Some(TableSetStatistics {
                    level_idx: select_level_id,
                    size_gb: 0f64,
                    cnt: 0,
                }),
                read_level_nplus1: Some(TableSetStatistics {
                    level_idx: target_level_id,
                    size_gb: 0f64,
                    cnt: 0,
                }),
                write: Some(TableSetStatistics {
                    level_idx: target_level_id,
                    size_gb: 0f64,
                    cnt: 0,
                }),
            }),
            task_status: false,
            // TODO: fill with compaction group info
            prefix_pairs: vec![],
        };
        self.next_compact_task_id += 1;
        Some(compact_task)
    }

    fn pick_compaction(&mut self, levels: Vec<Level>) -> Option<SearchResult> {
        // only support compact L0 to L1 or L0 to L0
        let picker = TierCompactionPicker::new(
            self.next_compact_task_id,
            Box::new(RangeOverlapStrategy::default()),
        );
        picker.pick_compaction(levels, &mut self.level_handlers)
    }

    /// Declares a task is either finished or canceled.
    pub fn report_compact_task(&mut self, compact_task: &CompactTask) {
        for level in &compact_task.input_ssts {
            self.level_handlers[level.level_idx as usize].remove_task(compact_task.task_id);
        }
    }

    /// Applies the compact task result and get a new hummock version.
    pub fn apply_compact_result(
        compact_task: &CompactTask,
        based_hummock_version: HummockVersion,
    ) -> HummockVersion {
        let mut new_version = based_hummock_version;
        new_version.safe_epoch = std::cmp::max(new_version.safe_epoch, compact_task.watermark);
        if compact_task.target_level == 0 {
            assert_eq!(compact_task.input_ssts[0].level_idx, 0);
            let mut new_table_infos = vec![];
            let mut find_remove_position = false;
            for (idx, table) in new_version.levels[0].table_infos.iter().enumerate() {
                if compact_task.input_ssts[0]
                    .table_infos
                    .iter()
                    .all(|stale| table.id != stale.id)
                {
                    new_table_infos.push(new_version.levels[0].table_infos[idx].clone());
                } else if !find_remove_position {
                    new_table_infos.extend(compact_task.sorted_output_ssts.clone());
                    find_remove_position = true;
                }
            }
            new_version.levels[compact_task.target_level as usize].table_infos = new_table_infos;
        } else {
            for (idx, input_level) in compact_task.input_ssts.iter().enumerate() {
                new_version.levels[idx].table_infos.retain(|sst| {
                    input_level
                        .table_infos
                        .iter()
                        .all(|stale| sst.id != stale.id)
                });
            }
            new_version.levels[compact_task.target_level as usize]
                .table_infos
                .extend(compact_task.sorted_output_ssts.clone());
            new_version.levels[compact_task.target_level as usize]
                .table_infos
                .sort_by(|sst1, sst2| {
                    let a = KeyRange::from(sst1.key_range.as_ref().unwrap());
                    let b = KeyRange::from(sst2.key_range.as_ref().unwrap());
                    a.cmp(&b)
                });
        }
        new_version
    }
}

impl Transactional for CompactStatus {
    fn upsert_in_transaction(&self, trx: &mut Transaction) -> Result<()> {
        trx.put(
            CompactStatus::cf_name().to_string(),
            CompactStatus::key().as_bytes().to_vec(),
            risingwave_pb::hummock::CompactStatus::from(self).encode_to_vec(),
        );
        Ok(())
    }

    fn delete_in_transaction(&self, trx: &mut Transaction) -> Result<()> {
        trx.delete(
            CompactStatus::cf_name().to_string(),
            CompactStatus::key().as_bytes().to_vec(),
        );
        Ok(())
    }
}

impl Default for CompactStatus {
    fn default() -> Self {
        Self::new()
    }
}

impl From<&CompactStatus> for risingwave_pb::hummock::CompactStatus {
    fn from(status: &CompactStatus) -> Self {
        risingwave_pb::hummock::CompactStatus {
            level_handlers: status.level_handlers.iter().map_into().collect(),
            next_compact_task_id: status.next_compact_task_id,
        }
    }
}

impl From<&risingwave_pb::hummock::CompactStatus> for CompactStatus {
    fn from(status: &risingwave_pb::hummock::CompactStatus) -> Self {
        CompactStatus {
            level_handlers: status.level_handlers.iter().map_into().collect(),
            next_compact_task_id: status.next_compact_task_id,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_serde() -> Result<()> {
        let mut origin = CompactStatus::new();
        origin.next_compact_task_id = 4;
        let ser = risingwave_pb::hummock::CompactStatus::from(&origin).encode_to_vec();
        let de = risingwave_pb::hummock::CompactStatus::decode(&mut Cursor::new(ser));
        let de = (&de.unwrap()).into();
        assert_eq!(origin, de);

        Ok(())
    }
}