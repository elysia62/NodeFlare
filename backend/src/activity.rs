use crate::routes::ApiResponse;
use axum::http::StatusCode;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::{RwLock, RwLockReadGuard, RwLockWriteGuard};

#[derive(Default)]
pub struct DatabaseActivity {
    access: RwLock<()>,
    restart_required: AtomicBool,
}

impl DatabaseActivity {
    pub fn read(&self) -> Result<RwLockReadGuard<'_, ()>, ApiResponse> {
        self.access.try_read().map_err(|_| {
            ApiResponse::error(
                StatusCode::SERVICE_UNAVAILABLE,
                "数据库正在维护，请稍后重试",
            )
        })
    }

    // Normal writers share access; maintenance drains and excludes all of them.
    pub fn write(&self) -> Result<RwLockReadGuard<'_, ()>, ApiResponse> {
        let access = self.read()?;
        self.ensure_writable()?;
        Ok(access)
    }

    pub async fn maintenance(&self) -> Result<RwLockWriteGuard<'_, ()>, ApiResponse> {
        self.maintenance_with_timeout(std::time::Duration::from_secs(30))
            .await
    }

    async fn maintenance_with_timeout(
        &self,
        timeout: std::time::Duration,
    ) -> Result<RwLockWriteGuard<'_, ()>, ApiResponse> {
        // Only bound the drain phase. Once destructive work starts, dropping its
        // guard while spawn_blocking continues would allow concurrent DB writes.
        let access = tokio::time::timeout(timeout, self.access.write())
            .await
            .map_err(|_| {
                ApiResponse::error(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "数据库正在维护，请稍后重试",
                )
            })?;
        self.ensure_writable()?;
        Ok(access)
    }

    fn ensure_writable(&self) -> Result<(), ApiResponse> {
        if self.restart_required() {
            return Err(ApiResponse::error(
                StatusCode::SERVICE_UNAVAILABLE,
                "数据库迁移已完成，请重启服务后继续修改",
            ));
        }
        Ok(())
    }

    pub fn require_restart(&self) {
        self.restart_required.store(true, Ordering::Release);
    }

    pub fn restart_required(&self) -> bool {
        self.restart_required.load(Ordering::Acquire)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::poll;
    use std::task::Poll;

    #[tokio::test]
    async fn maintenance_drains_writes_and_rejects_new_access() {
        let activity = DatabaseActivity::default();
        let writer = activity.write().unwrap();
        let maintenance = activity.maintenance();
        tokio::pin!(maintenance);
        assert!(matches!(poll!(&mut maintenance), Poll::Pending));
        assert!(activity.read().is_err());
        assert!(activity.write().is_err());
        drop(writer);
        let exclusive = maintenance.await.unwrap();
        assert!(activity.write().is_err());
        drop(exclusive);
        assert!(activity.write().is_ok());
    }

    #[tokio::test]
    async fn maintenance_timeout_cancels_queued_exclusivity_without_leaving_writes_blocked() {
        let activity = DatabaseActivity::default();
        let writer = activity.write().unwrap();
        assert!(
            activity
                .maintenance_with_timeout(std::time::Duration::from_millis(10))
                .await
                .is_err()
        );
        assert!(activity.write().is_ok());
        drop(writer);
        assert!(activity.maintenance().await.is_ok());
    }

    #[tokio::test]
    async fn migration_keeps_business_writes_frozen_until_restart() {
        let activity = DatabaseActivity::default();
        let exclusive = activity.maintenance().await.unwrap();
        activity.require_restart();
        drop(exclusive);
        assert!(activity.read().is_ok());
        assert!(activity.write().is_err());
        assert!(activity.maintenance().await.is_err());
    }
}
