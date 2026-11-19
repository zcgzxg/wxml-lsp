//! Frozen test fixture data for the assets-lock tests.

pub(crate) const ENGINE_ASSET: &str = "typescript-native-engine";
pub(crate) const TYPINGS_ASSET: &str = "miniprogram-api-typings";
pub(crate) const ENGINE_ROOT: &str = "engine";
pub(crate) const TYPINGS_ROOT: &str = "typings/miniprogram-api-typings";

pub(crate) const ENGINE_VERSION: &str = "7.1.0-beta";
pub(crate) const ENGINE_COMMIT: &str = "3e9236361fa97a9e6c685f4833ac457ae2d8c50a";
pub(crate) const ENGINE_URL: &str = "https://registry.npmjs.org/@typescript/typescript-linux-x64/-/typescript-linux-x64-7.1.0-beta.tgz";
pub(crate) const ENGINE_SRI: &str = "sha512-kcD51ihf+pnI5KT4uQQYG04OpwznaZYBlIQhDhl8fflFd27NktxXsUeguxZyvUMyWfum7IF2bBS6Jv7ox0AgFQ==";
pub(crate) const ENGINE_SHA256: &str =
    "59128685cb55f44c9e6644ef5edb3cb321618d9547b4ee94c3038598ffd51855";
pub(crate) const ENGINE_TSC_SHA256: &str =
    "a183b1956edbe7e7d98f2249858eedf2d8a9790809cc4909db1276e202faaa70";
pub(crate) const ENGINE_TSC_SIZE: u64 = 24_522_912;
pub(crate) const ENGINE_TARBALL_SIZE: u64 = 10_175_135;
pub(crate) const ENGINE_FILE_COUNT: u64 = 122;
pub(crate) const ENGINE_UNPACKED_SIZE: u64 = 28_450_796;
pub(crate) const ENGINE_STDLIB_COUNT: u64 = 116;
pub(crate) const ENGINE_STDLIB_BYTES: u64 = 3_870_196;
pub(crate) const ENGINE_STDLIB_DIGEST: &str =
    "5e126e7206065b6fff9d651eff3ad66b9413804d3e23d7ecfb4257299af3e85e";
pub(crate) const ENGINE_TREE_DIGEST: &str =
    "5ee874f4f3310cc5c80c4ec4296dafbbb0fd067bc0cb16751f757f5d40fd196d";

pub(crate) const TYPINGS_VERSION: &str = "5.3.1";
pub(crate) const TYPINGS_URL: &str =
    "https://registry.npmjs.org/miniprogram-api-typings/-/miniprogram-api-typings-5.3.1.tgz";
pub(crate) const TYPINGS_SRI: &str = "sha512-2CirO+C1z5RmKXDqTUSNbuyouHf5d0/KJTt+Qqi1EsiQYs8jlYhq7yWjp6EdTcJpvZzjuefZQCcDixYbrgIqow==";
pub(crate) const TYPINGS_UPSTREAM_COUNT: u64 = 26;
pub(crate) const TYPINGS_UPSTREAM_BYTES: u64 = 2_552_052;
pub(crate) const TYPINGS_PAYLOAD_COUNT: u64 = 23;
pub(crate) const TYPINGS_PAYLOAD_BYTES: u64 = 2_548_075;
pub(crate) const TYPINGS_PAYLOAD_DIGEST: &str =
    "ba3eb83410db79bd80727e9f38eef7cf0f0a3a3c7fde02f8d14467cf95bab46b";
pub(crate) const TYPINGS_UPSTREAM_26_DIGEST: &str =
    "2141aac47206441f0434a7b519e49da1e8756d1bcdb6c17809a7f5e188a34c42";
pub(crate) const TYPINGS_UPSTREAM_TREE_DIGEST: &str =
    "b89a8bc2d00e14d9b03719328093435dc8153ada5f79f2e7fb9f720b160caaed";
pub(crate) const TYPINGS_PAYLOAD_TREE_DIGEST: &str =
    "12a224f865880487f63b78d35f5e81835b1542c2bdb3a1e43bad0bc0eda7facb";
pub(crate) const TYPINGS_RUNTIME_COUNT: u64 = 20;
pub(crate) const TYPINGS_RUNTIME_BYTES: u64 = 2_522_752;
pub(crate) const TYPINGS_RUNTIME_DIGEST: &str =
    "821c93bfdf41324faa7ee44011a963993cfecc3b4ea9e41cd834e894755e9ba9";

pub(crate) const MINIMAL_CANONICAL_BYTES: &str =
    "{\n  \"schemaVersion\": 1,\n  \"assets\": []\n}\n";
pub(crate) const MINIMAL_LOCK_SHA256: &str =
    "427bc853754c83bccc1baa0275170692c65b512c29978ce7d0407037b3409aed";
pub(crate) const VALID_LOCK_SHA256: &str =
    "6d2415a0ce906546d77e00abff92f60b45e7b1c8445e71d6e3fc83e55bb51858";

pub(crate) const ENGINE_INCLUDE: [&str; 5] = [
    "lib/**",
    "LICENSE",
    "NOTICE.txt",
    "package.json",
    "README.md",
];
pub(crate) const TYPINGS_INCLUDE: [&str; 5] = [
    "index.d.ts",
    "types/**",
    "LICENSE",
    "README.md",
    "CHANGELOG.md",
];
pub(crate) const TYPINGS_EXCLUDED: [&str; 3] = ["package.json", "README-en.md", "typings.json"];

pub(crate) const TYPINGS_EXCLUDED_FILES: [(&str, &str, u64); 3] = [
    (
        "README-en.md",
        "0b239cf2de187501d8dbed123992ee91a026f431dd6892483af4ebfcebbc8de8",
        2_839,
    ),
    (
        "package.json",
        "7dc13675e2fed5daeb67ab90fe3a26d458883e7b6d1ce542d2b8048c40b6d1fc",
        1_077,
    ),
    (
        "typings.json",
        "0f11c8a7d974fcd773983554a9cc1fc69ce8adc71057d5613c6668e0a736653e",
        61,
    ),
];
