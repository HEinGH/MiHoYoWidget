//! 米游社 DS（Dynamic Secret）签名
//! 算法与 genshin.py 完全对齐；salt 随客户端版本更新，需持续跟进社区。
//!
//! ⚠️ v1.2.0-lite：本版本不请求战绩域，故 DS v2 与战绩域 salt 暂时标注 `allow(dead_code)`，
//!    保留供风控消退后恢复便签功能使用。

#[allow(dead_code)]
use rand::Rng;

/// 国服签到域 salt（genshin.py DS_SALT["cn_signin"]）
/// ⚠️ 米游社更新客户端版本时可能变更，需定期核对
pub const DS_SALT_CN_SIGNIN: &str = "LyD1rXqMv2GJhnwdvCBjFOKGiKuLY3aO";

/// 国服战绩域 salt（genshin.py DS_SALT[CHINESE]，game_record 用）—— 本版本未使用
#[allow(dead_code)]
pub const DS_SALT_CN_RECORD: &str = "xV8v4Qu54lUKrEYFZkJhB8cuOh9Asafs";

/// 绝区零战绩域 salt（genshin.py DS_SALT["cn_zzz"]，zzz 独立盐，与战绩域不同）—— 本版本未使用
#[allow(dead_code)]
pub const DS_SALT_CN_ZZZ: &str = "K9F1vLVTXtAQZBBUUMTQCQNpVfF4WJJ";

/// DS v1：用于签到域（event/bbs_sign_reward/*）
/// 返回 "t,r,hash"，作为请求头 `ds` 的值
pub fn ds_v1(salt: &str) -> String {
    let t = chrono::Utc::now().timestamp().to_string();
    let r: String = rand::thread_rng()
        .sample_iter(&rand::distributions::Uniform::from(0..10))
        .take(6)
        .map(|d| char::from(b'0' + d as u8))
        .collect();
    let hash = format!("{:x}", md5::compute(format!("{salt}&t={t}&r={r}")));
    format!("{t},{r},{hash}")
}

/// DS v2：用于战绩域（game_record/*）—— 本版本未使用，保留供恢复便签功能
/// 返回 "t,r,hash"；body/query 需按接口要求序列化
/// ⚠️ r 必须为 100001~200000（对齐 genshin.py generate_cn_dynamic_secret）
#[allow(dead_code)]
pub fn ds_v2(salt: &str, body: &str, query: &str) -> String {
    let t = chrono::Utc::now().timestamp().to_string();
    let r = rand::thread_rng().gen_range(100001..=200000).to_string();
    let hash = format!(
        "{:x}",
        md5::compute(format!("salt={salt}&t={t}&r={r}&b={body}&q={query}"))
    );
    format!("{t},{r},{hash}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ds_v1_format() {
        let ds = ds_v1(DS_SALT_CN_SIGNIN);
        let parts: Vec<&str> = ds.split(',').collect();
        assert_eq!(parts.len(), 3, "格式应为 t,r,hash");
        assert_eq!(parts[1].len(), 6, "随机数应为 6 位");
        assert_eq!(parts[2].len(), 32, "hash 应为 32 位 md5 hex");
        // 时间戳应接近当前时间
        let t: i64 = parts[0].parse().unwrap();
        let now = chrono::Utc::now().timestamp();
        assert!((now - t).abs() < 60);
    }
}
