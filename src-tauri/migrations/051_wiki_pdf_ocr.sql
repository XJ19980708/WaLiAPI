-- Wiki 项目新增 PDF 摄入视觉模型配置（用于扫描页/图文页 OCR）。
-- 与摄入模型平行：channel_id + model 双字段；均 NULL/空 = 不启用 PDF OCR 视觉识别，
-- 此时扫描版 PDF 摄入会返回 OCR_MODEL_NOT_CONFIGURED 提示。
--
-- 0.4.2 升级兼容：旧编号 048_wiki_pdf_ocr 重排为 051。旧库的 048 记录由启动时
-- db::backfill_reranked_migrations 映射注册为 051（视为已应用跳过），因此这里不会
-- 对已建列的升级库重复执行；新库正常执行建列。
ALTER TABLE wiki_projects ADD COLUMN pdf_ocr_channel_id TEXT;
ALTER TABLE wiki_projects ADD COLUMN pdf_ocr_model TEXT;
