import json
import sys

path, kind = sys.argv[1], sys.argv[2]
data = json.load(open(path))
if kind == "channels":
    # F-023 §1：`total` 是频道数，字段为 icon/id/kind/last_stop/name/programme_count/source_*
    print(1 if data.get("total", 0) >= 1 and data["channels"][0]["id"] == "c1" else 0)
elif kind == "programmes":
    # F-023 §2：没有 `count`，只有 `channel`/`programmes`/`tz_offset_hours`
    rows = data.get("programmes") or []
    print(1 if len(rows) == 1 and rows[0]["title"] == "新闻" else 0)
elif kind == "search":
    # F-023 §3：没有 `count`，命中在 `channels` 与 `programmes` 两个数组里
    print(
        1
        if len(data.get("programmes") or []) == 1
        and data["programmes"][0]["title"] == "新闻"
        else 0
    )
