//! A serial port as an RTU [`Line`], through the termios API (via
//! `rustix`, which is safe Rust over the syscalls: no `unsafe` here).
//!
//! The port is put in raw mode, 8 data bits, the configured parity and stop
//! bits, receiver on, modem lines ignored (`CLOCAL`), with reads that
//! return what is there (`VMIN` = `VTIME` = 0) after a `poll` for input.
//! It opens on first use and reopens after any error, so a USB adapter
//! that is unplugged and replugged comes back without a restart.

use std::fs::File;
use std::io::{self, Read, Write};
use std::time::Duration;

use rustix::event::{PollFd, PollFlags, Timespec, poll};
use rustix::fs::{Mode, OFlags};
use rustix::termios::{self, ControlModes, OptionalActions, QueueSelector, SpecialCodeIndex};

use super::{Line, Parity, Settings};

/// A serial port by path (`/dev/ttyUSB0`, `/dev/ttyAMA0`).
#[derive(Debug)]
pub struct SerialLine {
    path: String,
    settings: Settings,
    port: Option<File>,
}

impl SerialLine {
    #[must_use]
    pub fn new(path: &str, settings: Settings) -> Self {
        Self {
            path: path.into(),
            settings,
            port: None,
        }
    }

    fn port(&mut self) -> io::Result<&mut File> {
        if self.port.is_none() {
            // Non-blocking so the open itself never waits on a modem line;
            // blocking again once configured.
            let fd = rustix::fs::open(
                self.path.as_str(),
                OFlags::RDWR | OFlags::NOCTTY | OFlags::NONBLOCK | OFlags::CLOEXEC,
                Mode::empty(),
            )?;
            configure(&fd, self.settings)?;
            rustix::fs::fcntl_setfl(&fd, OFlags::empty())?;
            termios::tcflush(&fd, QueueSelector::IOFlush)?;
            self.port = Some(File::from(fd));
        }
        Ok(self.port.as_mut().expect("opened above"))
    }

    fn try_send(&mut self, frame: &[u8]) -> io::Result<()> {
        let port = self.port()?;
        port.write_all(frame)?;
        // Back only once the frame has left the UART, so the response
        // time-out starts when the request has been sent (Serial §2.4.1).
        termios::tcdrain(&*port)?;
        Ok(())
    }

    fn try_recv(&mut self, buf: &mut [u8], wait: Duration) -> io::Result<usize> {
        let port = self.port()?;
        let ts = Timespec::try_from(wait).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
        let mut fds = [PollFd::new(&*port, PollFlags::IN)];
        if poll(&mut fds, Some(&ts))? == 0 {
            return Ok(0);
        }
        let ev = fds[0].revents();
        if ev.intersects(PollFlags::ERR | PollFlags::HUP | PollFlags::NVAL) && !ev.contains(PollFlags::IN) {
            return Err(io::Error::new(io::ErrorKind::BrokenPipe, "serial port hung up"));
        }
        match port.read(buf)? {
            0 => Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "serial port went away",
            )),
            n => Ok(n),
        }
    }
}

fn configure(fd: &impl std::os::fd::AsFd, s: Settings) -> io::Result<()> {
    let mut t = termios::tcgetattr(fd)?;
    t.make_raw();
    t.set_speed(s.baud)?;
    let c = &mut t.control_modes;
    c.remove(ControlModes::CSIZE | ControlModes::PARENB | ControlModes::PARODD | ControlModes::CSTOPB);
    c.insert(ControlModes::CS8 | ControlModes::CREAD | ControlModes::CLOCAL);
    match s.parity {
        Parity::Even => c.insert(ControlModes::PARENB),
        Parity::Odd => c.insert(ControlModes::PARENB | ControlModes::PARODD),
        Parity::None => {}
    }
    if s.stop_bits == 2 {
        c.insert(ControlModes::CSTOPB);
    }
    t.special_codes[SpecialCodeIndex::VMIN] = 0;
    t.special_codes[SpecialCodeIndex::VTIME] = 0;
    termios::tcsetattr(fd, OptionalActions::Now, &t)?;
    Ok(())
}

impl Line for SerialLine {
    fn send(&mut self, frame: &[u8]) -> io::Result<()> {
        let r = self.try_send(frame);
        if r.is_err() {
            self.port = None;
        }
        r
    }

    fn recv(&mut self, buf: &mut [u8], wait: Duration) -> io::Result<usize> {
        let r = self.try_recv(buf, wait);
        if r.is_err() {
            self.port = None;
        }
        r
    }
}
