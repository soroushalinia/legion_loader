#![no_main]
#![no_std]
#![feature(abi_efiapi)]

use core::*;
extern crate alloc;
use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;
use log::error;
use uefi::prelude::*;
use uefi::proto::{console::gop::GraphicsOutput, media::file::*};
use uefi::table::boot::{AllocateType, MemoryType};
use xmas_elf::program::Type;
use xmas_elf::ElfFile;

pub struct FrameBuffer {
    _base_address: *mut u32,
    _size: usize,
    _width: usize,
    _height: usize,
    _stride: usize,
}

impl FrameBuffer {
    pub fn new(gop: *mut GraphicsOutput) -> Box<FrameBuffer> {
        let base_address = unsafe { (*gop).frame_buffer().as_mut_ptr() as *mut u32 };
        let size = unsafe { (*gop).frame_buffer().size() };
        let (width, height) = unsafe { (*gop).current_mode_info().resolution() };
        let stride = unsafe { (*gop).current_mode_info().stride() };
        let fb = FrameBuffer {
            _base_address: base_address,
            _size: size,
            _width: width,
            _height: height,
            _stride: stride,
        };

        let fb_heap = Box::new(fb);
        fb_heap
    }
}

#[entry]
fn main(handle: Handle, mut system_table: SystemTable<Boot>) -> Status {
    // Initialize UEFI services
    uefi_services::init(&mut system_table).unwrap();

    // Get systemtable pointer refrence for using fs uefi protocol
    let sys_table_fs = uefi_services::system_table().as_ptr();

    // Access simple file system protocol
    let sfs = unsafe {
        match BootServices::get_image_file_system((*sys_table_fs).boot_services(), handle) {
            Ok(sfs) => sfs,
            Err(why) => {
                error! {"{:?}",why};
                loop {}
            }
        }
    };

    // Open root directory
    let mut directory: Directory = unsafe { (*sfs.interface.get()).open_volume().unwrap() };

    // Open kernel file with name "legion_os"
    let kernel_path = cstr16!("legion_os");
    let kernel_handle = match directory.open(kernel_path, FileMode::Read, FileAttribute::empty()) {
        Ok(handle) => handle,
        Err(why) => {
            error!("Could not find kernel {:?}", why);
            loop {}
        }
    };

    let mut kernel_file = match kernel_handle.into_regular_file() {
        Some(reg_file) => reg_file,
        None => {
            error!("Kernel is not a file");
            loop {}
        }
    };

    // Get size of kernel and creating a buffer to read kernel info
    // If buffer size is not enough, kernel_file.gen_info() returns kernel size in err
    let mut kernel_info_buffer: Vec<u8> = Vec::new();
    let mut req_size = 0;
    match kernel_file.get_info::<FileInfo>(&mut kernel_info_buffer) {
        Ok(_) => (),
        Err(err) if err.status() == Status::BUFFER_TOO_SMALL => req_size = err.data().unwrap(),
        Err(why) => {
            error!("{:?}", why);
            loop {}
        }
    };
    // Resizing info buffer to actual kernel size
    kernel_info_buffer.resize(req_size, 0);

    // Get Kernel Info
    let kernel_info = match kernel_file.get_info::<FileInfo>(&mut kernel_info_buffer) {
        Ok(info) => info,
        Err(why) => {
            error!("{:#?}", why);
            loop {}
        }
    };

    // Get kernel size
    let kernel_size = kernel_info.file_size();
    // Create a buffer with size of kernel
    let mut kernel_buffer: Vec<u8> = vec![0; kernel_size.try_into().unwrap()];
    match kernel_file.read(&mut kernel_buffer) {
        Ok(_) => {}
        Err(why) => {
            error!("Failed to read kernel!:{:?}", why);
            loop {}
        }
    }

    // Parse ELF kernel File
    let kernel_elf = match ElfFile::new(&kernel_buffer) {
        Ok(elf) => elf,
        Err(why) => {
            error!("{}", why);
            loop {}
        }
    };

    // Set kernel entry point
    let entry_point = kernel_elf.header.pt2.entry_point();

    // Create a buffer for loaded sections since
    let mut loaded_sections: Vec<Vec<u8>> = Vec::new();

    for header in kernel_elf.program_iter() {
        if header.get_type().unwrap() == Type::Load {
            let virt_addr = header.virtual_addr();
            let file_size: usize = header.file_size().try_into().unwrap();
            let mem_size = header.mem_size();
            let file_offset: usize = header.offset().try_into().unwrap();

            // Align address on 4096 byte page size
            let address = virt_addr - (virt_addr % 4096);
            let mem_size_actual = (virt_addr - address) + mem_size;

            // Calculate number of pages required
            let num_pages: usize = ((mem_size_actual / 4096) + 1).try_into().unwrap();

            //Get pointer to system table for allocating page calls
            let table = uefi_services::system_table().as_ptr();

            // Allocate pages
            unsafe {
                let ptr = match BootServices::allocate_pages(
                    (*table).boot_services(),
                    AllocateType::Address(address.try_into().unwrap()),
                    MemoryType(2),
                    num_pages,
                ) {
                    Ok(addr) => addr,
                    Err(why) => {
                        error!("Failed to allocate pages:{:?}", why);
                        loop {}
                    }
                };

                //Create a vector from allocated buffer and fill it with zeros
                let mut buffer =
                    Vec::from_raw_parts(ptr as *mut u8, num_pages * 4096, num_pages * 4096);
                for byte in buffer.iter_mut() {
                    *byte = 0;
                }
                let offset: usize = (virt_addr - address).try_into().unwrap();
                let mut start_index: usize = (offset).try_into().unwrap();
                for byte in &kernel_buffer[file_offset..(file_offset + file_size)] {
                    buffer[start_index] = *byte;
                    start_index += 1;
                }
                loaded_sections.push(buffer);
            }
        }
    }

    // Set kernel function signature
    type KernelMain = fn(frame_buffer: &mut FrameBuffer, mem_map_buf: &mut [u8]) -> !;
    let kernel_main: KernelMain;
    unsafe {
        kernel_main = core::mem::transmute(entry_point);
    }

    // Get graphic output protocol
    let gop = BootServices::locate_protocol::<GraphicsOutput>(system_table.boot_services())
        .unwrap()
        .get();

    // Create a new framebuffer and leak it so it can be passed to kernel
    let framebuffer = FrameBuffer::new(gop);
    let framebuffer = Box::leak(framebuffer);

    // Get memory map size and create a buffer for memory map
    let size = BootServices::memory_map_size(&system_table.boot_services());
    let map_size = size.map_size;
    let entry_size = size.entry_size;
    let mut mem_map_buf: Vec<u8> = Vec::new();
    mem_map_buf.resize(map_size + entry_size * 10, 0);

    // Exit boot services
    system_table
        .exit_boot_services(handle, &mut mem_map_buf)
        .unwrap();
    kernel_main(framebuffer, &mut mem_map_buf);
}
