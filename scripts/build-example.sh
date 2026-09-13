#!/usr/bin/env bash
# 生成规范 §10 的完整示例 bundle（真实 payload + 真实 size/sha256）。
# 幂等：每次执行都会重建 examples/客户运营.str。
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="$ROOT/examples/客户运营.str"

ROOT_ID="01928f3a-7c4b-7000-8000-000000000000"
N1="01928f3a-7c4b-7001-8a01-000000000001"   # 客户档案
N2="01928f3a-7c4b-7002-8a02-000000000002"   # 订单数据集
N3="01928f3a-7c4b-7003-8a03-000000000003"   # 标签体系
L1="01928f3a-7c4b-7101-8b01-000000000101"   # 跟进记录（深度 2）
L2="01928f3a-7c4b-7102-8b02-000000000102"   # 2026-09 会议纪要（深度 3）
REF1="01928f3a-7c4b-7201-8d01-000000000201" # 关联线

hash_of() { shasum -a 256 "$1" | awk '{print $1}'; }
size_of() { wc -c < "$1" | tr -d ' '; }

rm -rf "$OUT"
mkdir -p "$OUT/._schema" \
         "$OUT/$N1/attachments" \
         "$OUT/$N1/$L1/$L2" \
         "$OUT/$N2" \
         "$OUT/$N3"

# ── bundle 级 Schema ────────────────────────────────────────────
cp "$ROOT"/schema/*.json "$OUT/._schema/"
python3 - "$OUT/._schema/customer.schema.json" <<'PY'
import json, sys
schema = {
    "$schema": "https://json-schema.org/draft/2020-12/schema",
    "$id": "https://str-format.local/example/customer.schema.json",
    "title": "crm.customer payload",
    "type": "object",
    "required": ["name", "region"],
    "additionalProperties": True,
    "properties": {
        "name": {"type": "string", "minLength": 1},
        "region": {"type": "string", "minLength": 1},
        "since": {"type": "string"},
        "tier": {"enum": ["normal", "vip"]},
    },
}
open(sys.argv[1], "w", encoding="utf-8").write(
    json.dumps(schema, ensure_ascii=False, indent=2) + "\n"
)
PY

# ── payload / asset ─────────────────────────────────────────────
printf '{\n  "name": "张伟",\n  "region": "华东",\n  "since": "2024-03-12",\n  "tier": "vip"\n}\n' \
  > "$OUT/$N1/profile.json"

python3 - "$OUT/$N1/avatar.png" <<'PY'
import base64, sys
# 1×1 透明 PNG
data = ("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk"
        "+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==")
open(sys.argv[1], "wb").write(base64.b64decode(data))
PY

printf '%%PDF-1.4\n1 0 obj<</Type/Catalog>>endobj\ntrailer<</Root 1 0 R>>\n%%%%EOF\n' \
  > "$OUT/$N1/attachments/合同-2024Q1.pdf"

printf '{\n  "items": [\n    {"at": "2026-09-02", "by": "Frowhy", "note": "首次需求沟通"},\n    {"at": "2026-09-10", "by": "Frowhy", "note": "产品对齐会议"}\n  ]\n}\n' \
  > "$OUT/$N1/$L1/followups.json"

printf '# 2026-09-10 会议纪要\n\n## 参会\n- 客户：张伟\n- 我方：Frowhy\n\n## 决议\n- 上线时间确定为 2026-10-08\n- 数据迁移由客户侧提供 CSV\n' \
  > "$OUT/$N1/$L1/$L2/2026-09-10.md"

printf 'order_id,sku,qty,amount,placed_at\nA-1001,SKU-01,2,398.00,2026-03-12\nA-1002,SKU-07,1,129.00,2026-04-02\nA-1003,SKU-01,5,995.00,2026-05-18\n' \
  > "$OUT/$N2/orders.csv"

printf '{\n  "tags": ["华东区", "vip", "制造业", "2024-新签"]\n}\n' \
  > "$OUT/$N3/tags.json"

# ── 各分支 `._meta` ─────────────────────────────────────────────
cat > "$OUT/._meta" <<EOF
# ── STR bundle 根元数据 ──────────────────────────────────────────
# ROOT 的 [[entries]] 中 role = "node" 的条目即一级分支结构。
str = 1
spec = "1.6.0"
kind = "root"
id = "$ROOT_ID"
name = "客户运营"
title = "客户运营结构化数据束"
summary = "以客户为主体的分支树，含跟进记录、订单与标签分支，供 CRM 与 AI 检索使用。"
tags = ["crm", "demo"]
revision = 12
created_at = 2026-09-01T09:12:00+08:00
updated_at = 2026-09-14T10:03:11+08:00

[policies]
id_version = 7
max_depth = 32
manifest = "strict"
sha256 = "required"
large_asset_bytes = 10485760
deep_tree_warn = 16

[[authors]]
id = "u:frowhy"
name = "Frowhy"
role = "owner"
at = 2026-09-01T09:12:00+08:00

[[authors]]
id = "u:agent-001"
name = "AI Agent"
role = "agent"
at = 2026-09-14T10:03:11+08:00

[[entries]]
path = "$N1"
role = "node"
id = "$N1"
type = "crm.customer"
title = "客户档案 · 张伟"
summary = "企业客户主体档案，含跟进记录分支。"
order = 1

[[entries]]
path = "$N2"
role = "node"
id = "$N2"
type = "crm.order_dataset"
title = "订单数据集"
summary = "全部订单明细。"
order = 2

[[entries]]
path = "$N3"
role = "node"
id = "$N3"
type = "crm.tag_system"
title = "标签体系"
summary = "客户与订单共用的标签字典。"
order = 3

[[entries]]
path = "._schema"
role = "schema"
count = 4
note = "bundle 级校验 Schema 存放处"

[ext]
EOF

PROFILE_SIZE=$(size_of "$OUT/$N1/profile.json");   PROFILE_HASH=$(hash_of "$OUT/$N1/profile.json")
AVATAR_SIZE=$(size_of "$OUT/$N1/avatar.png");      AVATAR_HASH=$(hash_of "$OUT/$N1/avatar.png")
FOLLOW_SIZE=$(size_of "$OUT/$N1/$L1/followups.json"); FOLLOW_HASH=$(hash_of "$OUT/$N1/$L1/followups.json")
MD_SIZE=$(size_of "$OUT/$N1/$L1/$L2/2026-09-10.md");  MD_HASH=$(hash_of "$OUT/$N1/$L1/$L2/2026-09-10.md")
CSV_SIZE=$(size_of "$OUT/$N2/orders.csv");         CSV_HASH=$(hash_of "$OUT/$N2/orders.csv")
TAGS_SIZE=$(size_of "$OUT/$N3/tags.json");         TAGS_HASH=$(hash_of "$OUT/$N3/tags.json")

cat > "$OUT/$N1/._meta" <<EOF
str = 1
spec = "1.6.0"
kind = "node"
id = "$N1"
type = "crm.customer"
title = "客户档案 · 张伟"
summary = "2024 年 3 月签约的企业客户，归属华东区，当前为 VIP 等级。"
tags = ["华东区", "vip"]
revision = 8
created_at = 2026-09-01T09:20:00+08:00
updated_at = 2026-09-14T10:03:11+08:00
schema = "._schema/customer.schema.json"

[[refs]]
id = "$REF1"
target = "$N2"
rel = "related"
title = "该客户的订单"
order = 1
note = "跨枝关联：客户档案 ⇢ 订单数据集"

[[entries]]
path = "profile.json"
role = "payload"
media_type = "application/json"
size = $PROFILE_SIZE
sha256 = "$PROFILE_HASH"
schema = "._schema/customer.schema.json"

[[entries]]
path = "avatar.png"
role = "asset"
media_type = "image/png"
size = $AVATAR_SIZE
sha256 = "$AVATAR_HASH"

[[entries]]
path = "attachments"
role = "dir"
count = 1
note = "合同扫描件；普通子目录，纯内容容器"

[[entries]]
path = "$L1"
role = "branch"
id = "$L1"
type = "crm.followup_log"
title = "跟进记录"
summary = "按时间的客户跟进日志，含下级会议纪要分支。"
order = 1

[ext]
EOF

cat > "$OUT/$N1/$L1/._meta" <<EOF
# 深度 2 的关联分支同样承载真实数据（payload 直接放在本目录内）
str = 1
spec = "1.6.0"
kind = "branch"
id = "$L1"
type = "crm.followup_log"
title = "跟进记录"
summary = "该客户的历次跟进摘要，含 2026-09 的会议纪要子分支。"
tags = ["followup"]
revision = 3
created_at = 2026-09-10T14:00:00+08:00
updated_at = 2026-09-14T10:03:11+08:00

[[entries]]
path = "followups.json"
role = "payload"
media_type = "application/json"
size = $FOLLOW_SIZE
sha256 = "$FOLLOW_HASH"

[[entries]]
path = "$L2"
role = "branch"
id = "$L2"
type = "doc.meeting_note"
title = "2026-09 会议纪要"
summary = "9 月与客户的产品对齐会议纪要。"
order = 1

[ext]
EOF

cat > "$OUT/$N1/$L1/$L2/._meta" <<EOF
str = 1
spec = "1.6.0"
kind = "branch"
id = "$L2"
type = "doc.meeting_note"
title = "2026-09 会议纪要"
summary = "深度 3 的关联分支，同样承载真实数据（Markdown 纪要）。"
tags = ["meeting"]
revision = 2
created_at = 2026-09-12T11:22:00+08:00
updated_at = 2026-09-13T09:05:00+08:00

[[entries]]
path = "2026-09-10.md"
role = "payload"
media_type = "text/markdown"
size = $MD_SIZE
sha256 = "$MD_HASH"

[ext]
EOF

cat > "$OUT/$N2/._meta" <<EOF
str = 1
spec = "1.6.0"
kind = "node"
id = "$N2"
type = "crm.order_dataset"
title = "订单数据集"
summary = "全部订单明细（CSV）。"
tags = ["order"]
revision = 4
created_at = 2026-09-02T10:00:00+08:00
updated_at = 2026-09-14T10:03:11+08:00

[[entries]]
path = "orders.csv"
role = "payload"
media_type = "text/csv"
size = $CSV_SIZE
sha256 = "$CSV_HASH"

[ext]
EOF

cat > "$OUT/$N3/._meta" <<EOF
str = 1
spec = "1.6.0"
kind = "node"
id = "$N3"
type = "crm.tag_system"
title = "标签体系"
summary = "客户与订单共用的标签字典。"
tags = ["taxonomy"]
revision = 2
created_at = 2026-09-03T15:30:00+08:00
updated_at = 2026-09-14T10:03:11+08:00

[[entries]]
path = "tags.json"
role = "payload"
media_type = "application/json"
size = $TAGS_SIZE
sha256 = "$TAGS_HASH"

[ext]
EOF

echo "已生成：$OUT"
"$ROOT/target/debug/str" tree "$OUT" --show-refs || true
