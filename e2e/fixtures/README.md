# API 响应 fixture

`real-visual.spec.ts` 用这些响应替换后端数据，使页面走真实（非 demo）代码路径，
同时保持数据确定性以便做像素比对。

## 重新录制

数据格式变化或需要换节点时，从真实后端重新抓取：

```bash
curl -s http://127.0.0.1:2206/api/bootstrap -o e2e/fixtures/bootstrap.json
```

## 注意

- **时间戳是录制时刻的，会过期。** 读入时须用 `anchoredBootstrap()` 把
  节点时间戳平移到当前，否则会因超出 `offline_threshold_seconds`
  被一律判为离线，两种状态看起来一样。
- 用例只依赖字段结构，不依赖具体数值，因此换一台机器录制也能用；
  但**数值本身的视觉宽度会影响像素**，更换 fixture 后需要同步更新基线。
