use anyhow::{bail, Context, Result};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{Cursor, Read, Write};
use std::path::Path;

// NBT标签类型码（基于您的描述和文献）
const TAG_END: u8 = 0x00;
const TAG_BYTE: u8 = 0x01;
const TAG_SHORT: u8 = 0x02;
const TAG_INT: u8 = 0x03;
const TAG_LONG: u8 = 0x04;
const TAG_FLOAT: u8 = 0x05;
const TAG_DOUBLE: u8 = 0x06;
const TAG_BYTE_ARRAY: u8 = 0x07;
const TAG_STRING: u8 = 0x08;
const TAG_LIST: u8 = 0x09;
const TAG_COMPOUND: u8 = 0x0A;
const TAG_INT_ARRAY: u8 = 0x0B;
const TAG_LONG_ARRAY: u8 = 0x0C;
const TAG_SHORT_ARRAY: u8 = 0x64; // 基于文献，可能特定于基岩版或扩展

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum NbtTag {
    End,
    Byte(i8),
    Short(i16),
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    ByteArray(Vec<i8>),
    String(String),
    List(Vec<NbtTag>),
    Compound(IndexMap<String, NbtTag>),
    IntArray(Vec<i32>),
    LongArray(Vec<i64>),
    ShortArray(Vec<i16>),
}

// 辅助函数：读取little-endian数值
fn read_le_u16(reader: &mut impl Read) -> Result<u16> {
    let mut buf = [0u8; 2];
    reader.read_exact(&mut buf)?;
    Ok(u16::from_le_bytes(buf))
}

fn read_le_i16(reader: &mut impl Read) -> Result<i16> {
    let mut buf = [0u8; 2];
    reader.read_exact(&mut buf)?;
    Ok(i16::from_le_bytes(buf))
}

fn read_le_i32(reader: &mut impl Read) -> Result<i32> {
    let mut buf = [0u8; 4];
    reader.read_exact(&mut buf)?;
    Ok(i32::from_le_bytes(buf))
}

fn read_le_i64(reader: &mut impl Read) -> Result<i64> {
    let mut buf = [0u8; 8];
    reader.read_exact(&mut buf)?;
    Ok(i64::from_le_bytes(buf))
}

fn read_le_f32(reader: &mut impl Read) -> Result<f32> {
    let mut buf = [0u8; 4];
    reader.read_exact(&mut buf)?;
    Ok(f32::from_le_bytes(buf))
}

fn read_le_f64(reader: &mut impl Read) -> Result<f64> {
    let mut buf = [0u8; 8];
    reader.read_exact(&mut buf)?;
    Ok(f64::from_le_bytes(buf))
}

// 读取字符串：u16 le长度 + UTF-8数据
fn read_string(reader: &mut impl Read) -> Result<String> {
    let len = read_le_u16(reader)? as usize;
    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf)?;
    String::from_utf8(buf).context("Invalid UTF-8 in string")
}

// 读取标签名：u16 le长度 + UTF-8
fn read_tag_name(reader: &mut impl Read) -> Result<String> {
    read_string(reader)
}

// 解析单个命名标签（type + name + payload）
fn parse_named_tag(reader: &mut impl Read) -> Result<(String, NbtTag)> {
    let tag_type = {
        let mut buf = [0u8; 1];
        reader.read_exact(&mut buf)?;
        buf[0]
    };

    if tag_type == TAG_END {
        return Ok(("".to_string(), NbtTag::End));
    }

    let name = read_tag_name(reader)?;
    let value = parse_tag_payload(reader, tag_type)?;
    Ok((name, value))
}

// 解析标签payload（根据类型）
fn parse_tag_payload(reader: &mut impl Read, tag_type: u8) -> Result<NbtTag> {
    match tag_type {
        TAG_BYTE => {
            let mut buf = [0u8; 1];
            reader.read_exact(&mut buf)?;
            Ok(NbtTag::Byte(buf[0] as i8))
        }
        TAG_SHORT => Ok(NbtTag::Short(read_le_i16(reader)?)),
        TAG_INT => Ok(NbtTag::Int(read_le_i32(reader)?)),
        TAG_LONG => Ok(NbtTag::Long(read_le_i64(reader)?)),
        TAG_FLOAT => Ok(NbtTag::Float(read_le_f32(reader)?)),
        TAG_DOUBLE => Ok(NbtTag::Double(read_le_f64(reader)?)),
        TAG_BYTE_ARRAY => {
            let len = read_le_i32(reader)? as usize;
            let mut buf = vec![0i8; len];
            reader.read_exact(unsafe {
                std::slice::from_raw_parts_mut(buf.as_mut_ptr() as *mut u8, len)
            })?;
            Ok(NbtTag::ByteArray(buf))
        }
        TAG_STRING => Ok(NbtTag::String(read_string(reader)?)),
        TAG_LIST => {
            let item_type = {
                let mut buf = [0u8; 1];
                reader.read_exact(&mut buf)?;
                buf[0]
            };
            let len = read_le_i32(reader)? as usize;
            let mut list = Vec::with_capacity(len);
            for _ in 0..len {
                list.push(parse_tag_payload(reader, item_type)?);
            }
            Ok(NbtTag::List(list))
        }
        TAG_COMPOUND => {
            let mut compound = IndexMap::new();
            loop {
                let (name, tag) = parse_named_tag(reader)?;
                if matches!(tag, NbtTag::End) {
                    break;
                }
                compound.insert(name, tag);
            }
            Ok(NbtTag::Compound(compound))
        }
        TAG_INT_ARRAY => {
            let len = read_le_i32(reader)? as usize;
            let mut arr = Vec::with_capacity(len);
            for _ in 0..len {
                arr.push(read_le_i32(reader)?);
            }
            Ok(NbtTag::IntArray(arr))
        }
        TAG_LONG_ARRAY => {
            let len = read_le_i32(reader)? as usize;
            let mut arr = Vec::with_capacity(len);
            for _ in 0..len {
                arr.push(read_le_i64(reader)?);
            }
            Ok(NbtTag::LongArray(arr))
        }
        TAG_SHORT_ARRAY => {
            let len = read_le_i32(reader)? as usize;
            let mut arr = Vec::with_capacity(len);
            for _ in 0..len {
                arr.push(read_le_i16(reader)?);
            }
            Ok(NbtTag::ShortArray(arr))
        }
        _ => bail!("Unknown NBT tag type: {}", tag_type),
    }
}

// 解析根NBT（通常为无名Compound）
pub fn parse_root_nbt(data: &[u8]) -> Result<NbtTag> {
    let mut cursor = Cursor::new(data);

    // 正确方式：根标签本身也有 type + name（通常是 TAG_Compound + ""），
    // 而不是直接进入 Compound payload。
    let (name, tag) = parse_named_tag(&mut cursor)?;

    if name.is_empty() {
        Ok(tag)
    } else {
        // 如果根有名字，就包裹在一个 Compound 中
        let mut root = IndexMap::new();
        root.insert(name, tag);
        Ok(NbtTag::Compound(root))
    }
}

// 序列化NBT到字节（little-endian）
fn write_le_u16(writer: &mut impl Write, val: u16) -> Result<()> {
    writer.write_all(&val.to_le_bytes())?;
    Ok(())
}

fn write_le_i16(writer: &mut impl Write, val: i16) -> Result<()> {
    writer.write_all(&val.to_le_bytes())?;
    Ok(())
}

fn write_le_i32(writer: &mut impl Write, val: i32) -> Result<()> {
    writer.write_all(&val.to_le_bytes())?;
    Ok(())
}

fn write_le_i64(writer: &mut impl Write, val: i64) -> Result<()> {
    writer.write_all(&val.to_le_bytes())?;
    Ok(())
}

fn write_le_f32(writer: &mut impl Write, val: f32) -> Result<()> {
    writer.write_all(&val.to_le_bytes())?;
    Ok(())
}

fn write_le_f64(writer: &mut impl Write, val: f64) -> Result<()> {
    writer.write_all(&val.to_le_bytes())?;
    Ok(())
}

fn write_string(writer: &mut impl Write, s: &str) -> Result<()> {
    let bytes = s.as_bytes();
    write_le_u16(writer, bytes.len() as u16)?;
    writer.write_all(bytes)?;
    Ok(())
}

fn serialize_named_tag(writer: &mut impl Write, name: &str, tag: &NbtTag) -> Result<()> {
    let tag_type = match tag {
        NbtTag::Byte(_) => TAG_BYTE,
        NbtTag::Short(_) => TAG_SHORT,
        NbtTag::Int(_) => TAG_INT,
        NbtTag::Long(_) => TAG_LONG,
        NbtTag::Float(_) => TAG_FLOAT,
        NbtTag::Double(_) => TAG_DOUBLE,
        NbtTag::ByteArray(_) => TAG_BYTE_ARRAY,
        NbtTag::String(_) => TAG_STRING,
        NbtTag::List(_) => TAG_LIST,
        NbtTag::Compound(_) => TAG_COMPOUND,
        NbtTag::IntArray(_) => TAG_INT_ARRAY,
        NbtTag::LongArray(_) => TAG_LONG_ARRAY,
        NbtTag::ShortArray(_) => TAG_SHORT_ARRAY,
        NbtTag::End => TAG_END,
    };
    writer.write_all(&[tag_type])?;
    if tag_type != TAG_END {
        write_string(writer, name)?;
    }
    serialize_tag_payload(writer, tag)?;
    Ok(())
}

fn serialize_tag_payload(writer: &mut impl Write, tag: &NbtTag) -> Result<()> {
    match tag {
        NbtTag::End => Ok(()),
        NbtTag::Byte(v) => Ok(writer.write_all(&[*v as u8])?),
        NbtTag::Short(v) => write_le_i16(writer, *v),
        NbtTag::Int(v) => write_le_i32(writer, *v),
        NbtTag::Long(v) => write_le_i64(writer, *v),
        NbtTag::Float(v) => write_le_f32(writer, *v),
        NbtTag::Double(v) => write_le_f64(writer, *v),
        NbtTag::ByteArray(arr) => {
            write_le_i32(writer, arr.len() as i32)?;
            // 安全转换 i8 slice -> u8 slice
            writer.write_all(unsafe {
                std::slice::from_raw_parts(arr.as_ptr() as *const u8, arr.len())
            })?;
            Ok(())
        }
        NbtTag::String(s) => write_string(writer, s),
        NbtTag::List(list) => {
            // 决定 item_type
            let item_type = if list.is_empty() {
                TAG_END
            } else {
                match &list[0] {
                    NbtTag::End => TAG_END,
                    NbtTag::Byte(_) => TAG_BYTE,
                    NbtTag::Short(_) => TAG_SHORT,
                    NbtTag::Int(_) => TAG_INT,
                    NbtTag::Long(_) => TAG_LONG,
                    NbtTag::Float(_) => TAG_FLOAT,
                    NbtTag::Double(_) => TAG_DOUBLE,
                    NbtTag::ByteArray(_) => TAG_BYTE_ARRAY,
                    NbtTag::String(_) => TAG_STRING,
                    NbtTag::List(_) => TAG_LIST,
                    NbtTag::Compound(_) => TAG_COMPOUND,
                    NbtTag::IntArray(_) => TAG_INT_ARRAY,
                    NbtTag::LongArray(_) => TAG_LONG_ARRAY,
                    NbtTag::ShortArray(_) => TAG_SHORT_ARRAY,
                }
            };
            writer.write_all(&[item_type])?;
            write_le_i32(writer, list.len() as i32)?;
            // 写入每个元素的 payload（注意：List 的元素是 payload-only，不含名字或类型字节）
            for item in list {
                serialize_tag_payload(writer, item)?;
            }
            Ok(())
        }
        NbtTag::Compound(map) => {
            for (name, value) in map {
                serialize_named_tag(writer, name, value)?;
            }
            writer.write_all(&[TAG_END])?; // 结束Compound
            Ok(())
        }
        NbtTag::IntArray(arr) => {
            write_le_i32(writer, arr.len() as i32)?;
            for &v in arr {
                write_le_i32(writer, v)?;
            }
            Ok(())
        }
        NbtTag::LongArray(arr) => {
            write_le_i32(writer, arr.len() as i32)?;
            for &v in arr {
                write_le_i64(writer, v)?;
            }
            Ok(())
        }
        NbtTag::ShortArray(arr) => {
            write_le_i32(writer, arr.len() as i32)?;
            for &v in arr {
                write_le_i16(writer, v)?;
            }
            Ok(())
        }
    }
}

// 序列化根NBT（无名Compound）
pub fn serialize_root_nbt(tag: &NbtTag) -> Result<Vec<u8>> {
    let mut buf = Vec::new();
    if let NbtTag::Compound(_) = tag {
        serialize_tag_payload(&mut buf, tag)?;
    } else {
        bail!("Root NBT must be Compound");
    }
    Ok(buf)
}

pub fn parse_root_nbt_with_header(data: &[u8]) -> Result<NbtTag> {
    if data.len() < 8 {
        bail!("数据不足，无法读取头部");
    }

    // 读取 version 和 declared_len
    let version = u32::from_le_bytes(data[0..4].try_into().unwrap());
    let declared_len = u32::from_le_bytes(data[4..8].try_into().unwrap()) as usize;
    let remaining = data.len().saturating_sub(8);

    tracing::debug!(
        "parse_root_nbt_with_header: version={} declared_len={} remaining={}",
        version,
        declared_len,
        remaining
    );

    // 截取 NBT 数据部分
    let nbt_data = if declared_len <= remaining {
        &data[8..8 + declared_len]
    } else {
        &data[8..]
    };

    // 调用原始解析
    parse_root_nbt(nbt_data)
}

/// 读取 level.dat 文件，专注于基岩版（无压缩）
pub fn read_level_dat(path: &Path) -> Result<NbtTag> {
    let bytes = fs::read(path).context("无法读取 level.dat 文件")?;

    // 使用新的解析逻辑
    parse_root_nbt_with_header(&bytes).context(format!("解析失败: {}", path.display()))
}

pub fn write_level_dat(path: &Path, tag: &NbtTag, version: u32) -> Result<()> {
    // 序列化 NBT（不压缩）
    let nbt_bytes = serialize_root_nbt(tag)?;
    let len = nbt_bytes.len() as u32;

    // 创建文件并写入
    let mut file = std::fs::File::create(path).context("Failed to create file")?;
    file.write_all(&version.to_le_bytes())?;
    file.write_all(&len.to_le_bytes())?;
    file.write_all(&nbt_bytes)?; // 直接写入原始数据（未压缩）
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    // 辅助：把字节切片打印成可读的十六进制字符串
    fn to_hex(b: &[u8]) -> String {
        b.iter()
            .map(|x| format!("{:02X}", x))
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[test]
    fn test_parse_simple_compound() {
        // 示例数据：无名Compound，包含一个StringTag "BiomeOverride" = ""
        let data = vec![
            0x08, 0x0D, 0x00, b'B', b'i', b'o', b'm', b'e', b'O', b'v', b'e', b'r', b'r', b'i',
            b'd', b'e', 0x00, 0x00, // StringTag, name len 13, name, value len 0
            0x00, // End
        ];

        // 解析并打印调试信息
        let tag = parse_root_nbt(&data).unwrap();
        eprintln!("parsed tag: {:#?}", tag); // 失败时会展示，若想总是看到，请用 --nocapture
        if let NbtTag::Compound(map) = tag {
            assert_eq!(
                map.get("BiomeOverride").unwrap(),
                &NbtTag::String("".to_string())
            );
        } else {
            panic!("Not compound");
        }
    }

    #[test]
    fn test_serialize_list() {
        // 示例：ListTag [1,2,3] 类型Int
        let list = NbtTag::List(vec![NbtTag::Int(1), NbtTag::Int(2), NbtTag::Int(3)]);
        let mut buf = Vec::new();
        serialize_named_tag(&mut buf, "test", &list).unwrap();

        // 调试输出：长度 + hex dump
        eprintln!("serialized bytes (len={}): {}", buf.len(), to_hex(&buf));

        // 断言各字段（和你期望的一一对应）
        assert_eq!(buf[0], TAG_LIST); // 09
        assert_eq!(&buf[1..3], &[0x04, 0x00]); // name len 4 (little-endian u16)
        assert_eq!(&buf[3..7], b"test"); // name
        assert_eq!(buf[7], TAG_INT); // item type 03 (TAG_INT)
        assert_eq!(&buf[8..12], &[0x03, 0x00, 0x00, 0x00]); // len 3 (i32, little-endian)

        // 打印值区段（方便核对每个 int 的字节序）
        eprintln!("values bytes: {}", to_hex(&buf[12..]));
        // 也可以做更严格的检查（例如解析后比较具体值）
        // 检查第一个 int（little-endian 4 字节 -> 1）
        let first_int = i32::from_le_bytes([buf[12], buf[13], buf[14], buf[15]]);
        assert_eq!(first_int, 1);
        let second_int = i32::from_le_bytes([buf[16], buf[17], buf[18], buf[19]]);
        assert_eq!(second_int, 2);
        let third_int = i32::from_le_bytes([buf[20], buf[21], buf[22], buf[23]]);
        assert_eq!(third_int, 3);
    }
}
