-- NULL means the account has not chosen a language preference yet.
ALTER TABLE users ADD COLUMN locale TEXT CHECK (locale IN ('auto', 'en', 'zh-Hans'));
