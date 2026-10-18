//! Reused software surfaces: opaque HWND for the card, premultiplied BGRA for
//! the transparent rail. No screen capture, color key, GPU loop or global opacity.
use std::{
    cell::RefCell,
    mem::size_of,
    ptr::{null, null_mut},
};
use windows::{
    Win32::Graphics::{
        Direct2D::{Common::*, *},
        Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM,
    },
    core::Interface,
};
use windows_sys::Win32::{
    Foundation::{HWND, POINT, SIZE},
    Graphics::Gdi::*,
    UI::WindowsAndMessaging::*,
};

enum Kind {
    Opaque(ID2D1HwndRenderTarget),
    Layered(ID2D1DCRenderTarget, RefCell<Option<Bitmap>>),
}
pub(super) struct Surface {
    hwnd: HWND,
    kind: Kind,
    pub target: ID2D1RenderTarget,
}
struct Bitmap {
    dc: HDC,
    bitmap: HBITMAP,
    old: HGDIOBJ,
    size: [u32; 2],
    #[cfg(test)]
    pixels: *mut u8,
}
impl Bitmap {
    fn new(size: [u32; 2]) -> windows::core::Result<Self> {
        unsafe {
            let dc = CreateCompatibleDC(null_mut());
            if dc.is_null() {
                return Err(windows::core::Error::from_thread());
            }
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: size[0] as i32,
                    biHeight: -(size[1] as i32),
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut pixels = null_mut();
            let bitmap = CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut pixels, null_mut(), 0);
            if bitmap.is_null() {
                let error = windows::core::Error::from_thread();
                DeleteDC(dc);
                return Err(error);
            }
            let old = SelectObject(dc, bitmap);
            Ok(Self {
                dc,
                bitmap,
                old,
                size,
                #[cfg(test)]
                pixels: pixels.cast(),
            })
        }
    }
}
impl Drop for Bitmap {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.old);
            DeleteObject(self.bitmap);
            DeleteDC(self.dc);
        }
    }
}
impl Surface {
    pub fn new(
        factory: &ID2D1Factory,
        hwnd: HWND,
        transparent: bool,
    ) -> windows::core::Result<Self> {
        let properties = D2D1_RENDER_TARGET_PROPERTIES {
            r#type: D2D1_RENDER_TARGET_TYPE_SOFTWARE,
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: if transparent {
                    D2D1_ALPHA_MODE_PREMULTIPLIED
                } else {
                    D2D1_ALPHA_MODE_IGNORE
                },
            },
            ..Default::default()
        };
        unsafe {
            let (kind, target) = if transparent {
                let dc = factory.CreateDCRenderTarget(&properties)?;
                (Kind::Layered(dc.clone(), RefCell::new(None)), dc.cast()?)
            } else {
                let target = factory.CreateHwndRenderTarget(
                    &properties,
                    &D2D1_HWND_RENDER_TARGET_PROPERTIES {
                        hwnd: windows::Win32::Foundation::HWND(hwnd),
                        pixelSize: D2D_SIZE_U {
                            width: 1,
                            height: 1,
                        },
                        presentOptions: D2D1_PRESENT_OPTIONS_NONE,
                    },
                )?;
                (Kind::Opaque(target.clone()), target.cast()?)
            };
            Ok(Self { hwnd, kind, target })
        }
    }
    pub fn prepare(&self, size: [u32; 2], dpi: u32) -> windows::core::Result<()> {
        let size = size.map(|n| n.max(1));
        unsafe {
            match &self.kind {
                Kind::Opaque(target) => {
                    let old = target.GetPixelSize();
                    if [old.width, old.height] != size {
                        target.Resize(&D2D_SIZE_U {
                            width: size[0],
                            height: size[1],
                        })?;
                    }
                }
                Kind::Layered(target, storage) => {
                    let mut storage = storage.borrow_mut();
                    if storage.as_ref().is_none_or(|b| b.size != size) {
                        *storage = Some(Bitmap::new(size)?);
                    }
                    let bitmap = storage.as_ref().unwrap();
                    target.BindDC(
                        windows::Win32::Graphics::Gdi::HDC(bitmap.dc),
                        &windows::Win32::Foundation::RECT {
                            left: 0,
                            top: 0,
                            right: size[0] as i32,
                            bottom: size[1] as i32,
                        },
                    )?;
                }
            }
            self.target.SetDpi(dpi as f32, dpi as f32);
            self.target
                .SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
            self.target.BeginDraw();
        }
        Ok(())
    }
    pub fn present(&self) -> windows::core::Result<()> {
        unsafe {
            self.target.EndDraw(None, None)?;
            if let Kind::Layered(_, storage) = &self.kind {
                let storage = storage.borrow();
                let bitmap = storage.as_ref().unwrap();
                let blend = BLENDFUNCTION {
                    BlendOp: AC_SRC_OVER as u8,
                    BlendFlags: 0,
                    SourceConstantAlpha: 255,
                    AlphaFormat: AC_SRC_ALPHA as u8,
                };
                if UpdateLayeredWindow(
                    self.hwnd,
                    null_mut(),
                    null(),
                    &SIZE {
                        cx: bitmap.size[0] as i32,
                        cy: bitmap.size[1] as i32,
                    },
                    bitmap.dc,
                    &POINT { x: 0, y: 0 },
                    0,
                    &blend,
                    ULW_ALPHA,
                ) == 0
                {
                    return Err(windows::core::Error::from_thread());
                }
            }
        }
        Ok(())
    }
    #[cfg(test)]
    pub fn pixels(&self) -> Vec<u8> {
        let Kind::Layered(_, storage) = &self.kind else {
            panic!("not layered")
        };
        let storage = storage.borrow();
        let b = storage.as_ref().unwrap();
        unsafe {
            std::slice::from_raw_parts(b.pixels, b.size[0] as usize * b.size[1] as usize * 4)
                .to_vec()
        }
    }
}
