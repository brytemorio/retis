/* automatically generated by rust-bindgen 0.70.1 */

pub type __u8 = ::std::os::raw::c_uchar;
pub type __u16 = ::std::os::raw::c_ushort;
pub type __u32 = ::std::os::raw::c_uint;
pub type u8_ = __u8;
pub type u16_ = __u16;
pub type u32_ = __u32;
pub const SECTION_META: ct_sections = 0;
pub const SECTION_BASE_CONN: ct_sections = 1;
pub const SECTION_PARENT_CONN: ct_sections = 2;
pub type ct_sections = ::std::os::raw::c_uint;
pub const RETIS_CT_DIR_ORIG: ct_flags = 1;
pub const RETIS_CT_DIR_REPLY: ct_flags = 2;
pub const RETIS_CT_IPV4: ct_flags = 4;
pub const RETIS_CT_IPV6: ct_flags = 8;
pub const RETIS_CT_PROTO_TCP: ct_flags = 16;
pub const RETIS_CT_PROTO_UDP: ct_flags = 32;
pub const RETIS_CT_PROTO_ICMP: ct_flags = 64;
pub type ct_flags = ::std::os::raw::c_uint;
#[repr(C)]
#[derive(Debug, Default, Copy, Clone)]
pub struct ct_meta_event {
    pub state: u8_,
}
#[repr(C)]
#[derive(Copy, Clone)]
pub union nf_conn_ip {
    pub ipv4: u32_,
    pub ipv6: [u8_; 16usize],
}
impl Default for nf_conn_ip {
    fn default() -> Self {
        let mut s = ::std::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::std::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}
#[repr(C)]
#[derive(Copy, Clone)]
pub struct nf_conn_addr_proto {
    pub addr: nf_conn_ip,
    pub data: u16_,
}
impl Default for nf_conn_addr_proto {
    fn default() -> Self {
        let mut s = ::std::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::std::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}
#[repr(C)]
#[derive(Copy, Clone)]
pub struct nf_conn_tuple {
    pub src: nf_conn_addr_proto,
    pub dst: nf_conn_addr_proto,
}
impl Default for nf_conn_tuple {
    fn default() -> Self {
        let mut s = ::std::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::std::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}
#[repr(C)]
#[derive(Copy, Clone)]
pub struct ct_event {
    pub orig: nf_conn_tuple,
    pub reply: nf_conn_tuple,
    pub flags: u32_,
    pub mark: u32_,
    pub zone_id: u16_,
    pub tcp_state: u8_,
}
impl Default for ct_event {
    fn default() -> Self {
        let mut s = ::std::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::std::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}
