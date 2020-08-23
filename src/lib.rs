use crossbeam_queue::ArrayQueue;
use pyo3::prelude::*;
use pyo3::types::PyByteArray;
use std::net::{SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{sleep, spawn as spawn_thread, JoinHandle};
use std::time::Duration;

pub const LOOP_GRACE_DURATION_MS: u64 = 1;
pub const MAX_PENDING_TX: usize = 128;
pub const MAX_PENDING_RX: usize = 128;
pub const MAX_PACKET_SIZE: usize = 1460;

#[pyclass(freelist = 1024)]
#[derive(Clone, Debug)]
pub struct UdpPacket {
    pub peer: SocketAddr,
    pub data: Vec<u8>,
}

#[pymethods]
impl UdpPacket {
    #[new]
    pub fn new(peer_ip_address: &str, peer_port: u16) -> Self {
        Self {
            peer: SocketAddr::new(peer_ip_address.parse().unwrap(), peer_port),
            data: Vec::new(),
        }
    }

    #[getter]
    pub fn get_data<'a>(&self, py: Python<'a>) -> PyResult<&'a PyByteArray> {
        Ok(PyByteArray::new(py, &self.data[..]))
    }

    #[setter]
    pub fn set_data(&mut self, raw_bytes: &PyByteArray) -> PyResult<()> {
        self.data = raw_bytes.to_vec();

        Ok(())
    }
}

#[pyclass]
pub struct UdpLoop {
    stop_flag: Arc<AtomicBool>,
    pending_tx: Arc<ArrayQueue<UdpPacket>>,
    pending_rx: Arc<ArrayQueue<UdpPacket>>,
    loop_handle: Mutex<Option<JoinHandle<()>>>,
}

impl Default for UdpLoop {
    fn default() -> Self {
        Self {
            stop_flag: Arc::new(AtomicBool::new(false)),
            pending_tx: Arc::new(ArrayQueue::new(MAX_PENDING_TX)),
            pending_rx: Arc::new(ArrayQueue::new(MAX_PENDING_RX)),
            loop_handle: Mutex::new(None),
        }
    }
}

#[pymethods]
impl UdpLoop {
    #[new]
    pub fn new() -> Self {
        Default::default()
    }

    pub fn try_receive(&self) -> PyResult<Option<UdpPacket>> {
        match self.pending_rx.pop() {
            Err(_) => Ok(None),
            Ok(packet) => Ok(Some(packet)),
        }
    }

    pub fn transmit(&self, packet: UdpPacket) -> PyResult<bool> {
        if let Err(_) = self.pending_tx.push(packet) {
            return Ok(false);
        }

        Ok(true)
    }

    pub fn start(&mut self, listen_address: &str, listen_port: u16) -> PyResult<bool> {
        let stop_flag = self.stop_flag.clone();
        let pending_tx = self.pending_tx.clone();
        let pending_rx = self.pending_rx.clone();
        let listen_address = listen_address.to_string();
        let mut loop_handle = self.loop_handle.lock().unwrap();

        if loop_handle.is_some() {
            return Ok(false);
        }

        (*loop_handle) = Some(spawn_thread(move || {
            let loop_grace_duration = Duration::from_millis(LOOP_GRACE_DURATION_MS);
            let mut buffer_rx = [0u8; MAX_PACKET_SIZE];
            let binding_socket = SocketAddr::new(listen_address.parse().unwrap(), listen_port);
            let udp_socket = UdpSocket::bind(binding_socket).unwrap();
            udp_socket.set_nonblocking(true).unwrap();
            let pending_tx = pending_tx;
            let pending_rx = pending_rx;

            while !stop_flag.load(Ordering::Relaxed) {
                if let Ok(new_tx_packet) = pending_tx.pop() {
                    let _ = udp_socket.send_to(&new_tx_packet.data, new_tx_packet.peer);
                }

                match udp_socket.recv_from(&mut buffer_rx) {
                    Err(_) => {
                        if pending_tx.is_empty() {
                            sleep(loop_grace_duration);
                        }
                    }
                    Ok((rx_length, peer)) => {
                        if rx_length > buffer_rx.len() {
                            eprintln!("Received packet beyond maximum size of {}", MAX_PACKET_SIZE);
                        } else {
                            let new_rx_packet = UdpPacket {
                                peer,
                                data: (&buffer_rx[0..rx_length]).to_vec(),
                            };
                            let _ = pending_rx.push(new_rx_packet);
                        }
                    }
                }
            }
        }));

        Ok(true)
    }

    pub fn stop(&mut self) -> PyResult<bool> {
        match self.loop_handle.lock().unwrap().take() {
            None => Ok(false),
            Some(loop_handle) => {
                self.stop_flag.store(true, Ordering::Relaxed);
                let _ = loop_handle.join();
                Ok(true)
            }
        }
    }
}

#[pymodule]
fn py_udp_loop(_: Python, m: &PyModule) -> PyResult<()> {
    m.add_class::<UdpPacket>()?;
    m.add_class::<UdpLoop>()?;

    Ok(())
}
