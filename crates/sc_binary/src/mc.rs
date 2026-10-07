//! Minecraft-specific binary types: serialization bridge between game structs
//! (Gamemode/GameRules/Motd/experiments) and `ByteReader`/`ByteWriter`.

use crate::interfaces::{Reader, Writer};
use crate::{ByteReader, ByteWriter};
use std::io::{Error, ErrorKind};
use sc_utils::game::experiment::ExperimentData;
use sc_utils::game::gamemode::Gamemode;
use sc_utils::game::gamerules::{GameRuleType, GameRuleValue};
use sc_utils::game::structs::motd::Motd;

impl Reader<Motd> for Motd {
    fn read(buf: &mut ByteReader) -> Result<Motd, Error> {
        let str_len = buf.read_u16()?;
        let mut str_buf = vec![0; str_len as usize];

        buf.read(&mut str_buf)?;

        let motd = String::from_utf8(str_buf)
            .map_err(|error| Error::new(ErrorKind::InvalidData, error))?;

        let parts = motd
            .split(";")
            .map(|c| c.to_string())
            .collect::<Vec<String>>();

        let motd = parts
            .get(1)
            .ok_or(Error::new(
                std::io::ErrorKind::InvalidData,
                "Invalid motd name",
            ))?
            .clone();

        let protocol = parts
            .get(2)
            .ok_or(Error::new(
                std::io::ErrorKind::InvalidData,
                "Invalid motd protocol",
            ))?
            .clone();

        let version = parts
            .get(3)
            .ok_or(Error::new(
                std::io::ErrorKind::InvalidData,
                "Invalid motd version",
            ))?
            .clone();

        let player_online = parts
            .get(4)
            .ok_or(Error::new(
                std::io::ErrorKind::InvalidData,
                "Invalid motd player online",
            ))?
            .clone();

        let player_max = parts
            .get(5)
            .ok_or(Error::new(
                std::io::ErrorKind::InvalidData,
                "Invalid motd player max",
            ))?
            .clone();

        let server_guid = parts
            .get(6)
            .ok_or(Error::new(
                std::io::ErrorKind::InvalidData,
                "Invalid motd server guid",
            ))?
            .clone();

        let level_name = parts
            .get(7)
            .ok_or(Error::new(
                std::io::ErrorKind::InvalidData,
                "Invalid level name",
            ))?
            .clone();

        let gamemode = parts
            .get(8)
            .ok_or(Error::new(
                std::io::ErrorKind::InvalidData,
                "Invalid motd gamemode",
            ))?
            .clone();

        Ok(Motd {
            motd,
            protocol: protocol
                .as_str()
                .parse()
                .map_err(|error| Error::new(ErrorKind::InvalidData, error))?,
            version,
            player_online: player_online
                .parse()
                .map_err(|error| Error::new(ErrorKind::InvalidData, error))?,
            player_max: player_max
                .parse()
                .map_err(|error| Error::new(ErrorKind::InvalidData, error))?,
            server_guid: server_guid
                .parse()
                .map_err(|error| Error::new(ErrorKind::InvalidData, error))?,
            level_name,
            gamemode: match gamemode
                .as_str()
                .parse::<u8>()
                .map_err(|error| Error::new(ErrorKind::InvalidData, error))?
            {
                0 => Gamemode::Survival,
                1 => Gamemode::Creative,
                2 => Gamemode::Adventure,
                3 => Gamemode::Spectator,
                _ => Gamemode::Survival,
            },
        })
    }
}

impl Writer for Motd {
    fn write(&self, buf: &mut ByteWriter) -> Result<(), Error> {
        let motd = self.write();
        let motd_len = u16::try_from(motd.len()).map_err(|_| {
            Error::new(
                ErrorKind::InvalidData,
                "MOTD is too long to encode with u16 length",
            )
        })?;
        buf.write_u16(motd_len)?;
        buf.write(motd.as_bytes())?;
        Ok(())
    }
}

impl Writer for ExperimentData {
    fn write(&self, buf: &mut ByteWriter) -> Result<(), Error> {
        buf.write_string(self.name)?;
        buf.write_bool(self.is_enabled)?;
        Ok(())
    }
}

impl Writer for GameRuleType {
    fn write(&self, buf: &mut ByteWriter) -> Result<(), Error> {
        match self {
            GameRuleType::Unknown => Ok(()),
            GameRuleType::Bool(value) => buf.write_bool(*value),
            GameRuleType::Int(value) => buf.write_i32_le(*value), // LInt (little-endian) for non-startGame mode
            GameRuleType::Float(value) => buf.write_f32_le(*value), // LFloat (little-endian)
        }
    }
}

impl Writer for GameRuleValue {
    fn write(&self, buf: &mut ByteWriter) -> Result<(), Error> {
        buf.write_bool(self.can_be_changed)?;
        buf.write_var_u32(self.value.index() as u32)?;
        self.value.write(buf)
    }
}
