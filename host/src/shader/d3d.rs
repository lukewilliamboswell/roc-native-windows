use super::{FrameSize, Sampling};
use crate::Result;
use crate::raster::Pixels;
use crate::shader_bytecode::Reflection;
use std::collections::BTreeMap;
use std::sync::Arc;
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP, D3D_FEATURE_LEVEL_11_0,
    D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST,
};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::IDXGIAdapter;

const VERTEX_SHADER: &[u8] = include_bytes!("../../shaders/fullscreen.cso");

pub(super) struct Gpu {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    vertex: ID3D11VertexShader,
    point: ID3D11SamplerState,
    linear: ID3D11SamplerState,
}

#[derive(Default)]
pub(super) struct ViewGpu {
    program: Option<(Arc<Program>, Shader)>,
    source: [Option<Texture>; 2],
    target: Option<Target>,
}

pub(super) struct Program {
    pub bytes: Vec<u8>,
    pub reflection: Reflection,
}

struct Shader {
    pixel: ID3D11PixelShader,
    constants: Option<ID3D11Buffer>,
}

struct Texture {
    pixels: Arc<Pixels>,
    resource: ID3D11ShaderResourceView,
}

struct Target {
    size: FrameSize,
    texture: ID3D11Texture2D,
    color: ID3D11RenderTargetView,
    staging: ID3D11Texture2D,
}

impl Gpu {
    pub(super) fn new() -> Result<Self> {
        let mut failures = Vec::new();
        for (name, driver) in [
            ("hardware", D3D_DRIVER_TYPE_HARDWARE),
            ("WARP", D3D_DRIVER_TYPE_WARP),
        ] {
            let mut device = None;
            let mut context = None;
            // SAFETY: Outputs live across creation and all interfaces are confined
            // to the worker's one rendering thread.
            let result = unsafe {
                D3D11CreateDevice(
                    None::<&IDXGIAdapter>,
                    driver,
                    HMODULE::default(),
                    D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                    Some(&[D3D_FEATURE_LEVEL_11_0]),
                    D3D11_SDK_VERSION,
                    Some(&mut device),
                    None,
                    Some(&mut context),
                )
            };
            match result {
                Ok(()) => {
                    return Self::from_device(
                        device.ok_or("Direct3D returned no device")?,
                        context.ok_or("Direct3D returned no immediate context")?,
                    );
                }
                Err(err) => failures.push(format!("{name}: {err}")),
            }
        }
        Err(format!(
            "Direct3D 11 is unavailable ({})",
            failures.join(", ")
        ))
    }

    fn from_device(device: ID3D11Device, context: ID3D11DeviceContext) -> Result<Self> {
        let mut vertex = None;
        // SAFETY: The platform's precompiled vertex shader and output are live.
        unsafe {
            device.CreateVertexShader(
                VERTEX_SHADER,
                None::<&ID3D11ClassLinkage>,
                Some(&mut vertex),
            )
        }
        .map_err(|err| format!("create the fullscreen vertex shader: {err}"))?;
        let point = sampler(&device, D3D11_FILTER_MIN_MAG_MIP_POINT)?;
        let linear = sampler(&device, D3D11_FILTER_MIN_MAG_LINEAR_MIP_POINT)?;
        Ok(Self {
            device,
            context,
            vertex: vertex.ok_or("missing fullscreen vertex shader")?,
            point,
            linear,
        })
    }

    pub(super) fn removed(&self) -> bool {
        // SAFETY: This interface is used only on the graphics worker.
        unsafe { self.device.GetDeviceRemovedReason().is_err() }
    }

    pub(super) fn draw(
        &self,
        state: &mut ViewGpu,
        program: &Arc<Program>,
        sources: &[Option<Arc<Pixels>>; 2],
        parameters: &BTreeMap<String, f32>,
        sampling: Sampling,
        size: FrameSize,
    ) -> Result<Arc<Pixels>> {
        let constants = program.reflection.constants(parameters)?;
        if !state
            .program
            .as_ref()
            .is_some_and(|(old, _)| Arc::ptr_eq(old, program))
        {
            state.program = Some((program.clone(), self.pixel_shader(program)?));
        }
        for (index, (texture, wanted)) in state.source.iter_mut().zip(sources).enumerate() {
            if !program.reflection.textures[index] {
                *texture = None;
                continue;
            }
            let source = wanted
                .as_ref()
                .ok_or_else(|| format!("shader requires texture slot t{index}"))?;
            if !texture
                .as_ref()
                .is_some_and(|old| Arc::ptr_eq(&old.pixels, source))
            {
                *texture = Some(self.upload(source.clone())?);
            }
        }
        if !state
            .target
            .as_ref()
            .is_some_and(|target| target.size == size)
        {
            state.target = Some(self.target(size)?);
        }
        let target = state.target.as_ref().unwrap();
        let shader = &state.program.as_ref().unwrap().1;
        let inputs = state
            .source
            .each_ref()
            .map(|slot| slot.as_ref().map(|texture| texture.resource.clone()));
        let sampler = match sampling {
            Sampling::Nearest => &self.point,
            Sampling::Linear => &self.linear,
        };
        let viewport = D3D11_VIEWPORT {
            Width: size.width as f32,
            Height: size.height as f32,
            MinDepth: 0.0,
            MaxDepth: 1.0,
            ..D3D11_VIEWPORT::default()
        };
        // SAFETY: All bindings are owned by this device and stay live while
        // the worker uses its serialized context. The reflected cbuffer spans
        // precisely the bytes passed to UpdateSubresource.
        unsafe {
            let ctx = &self.context;
            if let Some(buffer) = &shader.constants {
                ctx.UpdateSubresource(buffer, 0, None, constants.as_ptr().cast(), 0, 0);
            }
            ctx.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            ctx.VSSetShader(&self.vertex, None);
            ctx.PSSetShader(&shader.pixel, None);
            ctx.PSSetConstantBuffers(0, Some(std::slice::from_ref(&shader.constants)));
            ctx.PSSetShaderResources(0, Some(&inputs));
            ctx.PSSetSamplers(0, Some(&[Some(sampler.clone())]));
            ctx.RSSetViewports(Some(&[viewport]));
            ctx.OMSetRenderTargets(
                Some(&[Some(target.color.clone())]),
                None::<&ID3D11DepthStencilView>,
            );
            ctx.ClearRenderTargetView(&target.color, &[0.0, 0.0, 0.0, 0.0]);
            ctx.Draw(3, 0);
            ctx.PSSetShaderResources(0, Some(&[None, None]));
            ctx.PSSetConstantBuffers(0, Some(&[None]));
            ctx.PSSetShader(None::<&ID3D11PixelShader>, None);
            ctx.OMSetRenderTargets(None, None::<&ID3D11DepthStencilView>);
            ctx.CopyResource(&target.staging, &target.texture);
        }
        self.readback(target)
    }

    fn pixel_shader(&self, program: &Program) -> Result<Shader> {
        let mut pixel = None;
        let mut constants = None;
        // SAFETY: Both output slots are writable, and compiled input remains live.
        unsafe {
            self.device
                .CreatePixelShader(
                    &program.bytes,
                    None::<&ID3D11ClassLinkage>,
                    Some(&mut pixel),
                )
                .map_err(|err| format!("create pixel shader: {err}"))?;
            if program.reflection.width != 0 {
                let desc = D3D11_BUFFER_DESC {
                    ByteWidth: program.reflection.width as u32,
                    Usage: D3D11_USAGE_DEFAULT,
                    BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
                    ..D3D11_BUFFER_DESC::default()
                };
                self.device
                    .CreateBuffer(&desc, None, Some(&mut constants))
                    .map_err(|err| format!("create shader constants: {err}"))?;
            }
        }
        Ok(Shader {
            pixel: pixel.ok_or("missing Direct3D pixel shader")?,
            constants,
        })
    }

    fn upload(&self, pixels: Arc<Pixels>) -> Result<Texture> {
        let size = FrameSize {
            width: pixels.width,
            height: pixels.height,
        };
        let desc = D3D11_TEXTURE2D_DESC {
            Usage: D3D11_USAGE_IMMUTABLE,
            BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
            ..texture_desc(size)
        };
        let data = D3D11_SUBRESOURCE_DATA {
            pSysMem: pixels.bgra.as_ptr().cast(),
            SysMemPitch: size.width * 4,
            SysMemSlicePitch: 0,
        };
        let mut texture = None;
        let mut resource = None;
        // SAFETY: Raster dimensions and strides were checked by the decoder.
        // CreateTexture2D copies the immutable data synchronously.
        unsafe {
            self.device
                .CreateTexture2D(&desc, Some(&data), Some(&mut texture))
                .map_err(|err| format!("upload shader texture: {err}"))?;
            self.device
                .CreateShaderResourceView(
                    texture.as_ref().ok_or("missing Direct3D texture")?,
                    None,
                    Some(&mut resource),
                )
                .map_err(|err| format!("bind shader texture: {err}"))?;
        }
        Ok(Texture {
            pixels,
            resource: resource.ok_or("missing Direct3D texture view")?,
        })
    }

    fn target(&self, size: FrameSize) -> Result<Target> {
        let output = D3D11_TEXTURE2D_DESC {
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_RENDER_TARGET.0 as u32,
            ..texture_desc(size)
        };
        let staging = D3D11_TEXTURE2D_DESC {
            Usage: D3D11_USAGE_STAGING,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
            ..texture_desc(size)
        };
        let mut texture = None;
        let mut readback = None;
        let mut color = None;
        // SAFETY: The bounded descriptors and writable outputs are live.
        unsafe {
            self.device
                .CreateTexture2D(&output, None, Some(&mut texture))
                .map_err(|err| format!("create shader color target: {err}"))?;
            self.device
                .CreateRenderTargetView(
                    texture.as_ref().ok_or("missing Direct3D color texture")?,
                    None,
                    Some(&mut color),
                )
                .map_err(|err| format!("create shader color view: {err}"))?;
            self.device
                .CreateTexture2D(&staging, None, Some(&mut readback))
                .map_err(|err| format!("create shader readback texture: {err}"))?;
        }
        Ok(Target {
            size,
            texture: texture.unwrap(),
            color: color.ok_or("missing Direct3D color view")?,
            staging: readback.ok_or("missing Direct3D staging texture")?,
        })
    }

    fn readback(&self, target: &Target) -> Result<Arc<Pixels>> {
        let FrameSize { width, height } = target.size;
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        let row_bytes = width as usize * 4;
        // SAFETY: The staging surface has CPU read access and contains the
        // copied target; it remains mapped only while we copy the rows.
        unsafe {
            self.context
                .Map(&target.staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
                .map_err(|err| format!("read back shader frame: {err}"))?;
            if mapped.pData.is_null() || (mapped.RowPitch as usize) < row_bytes {
                self.context.Unmap(&target.staging, 0);
                return Err("invalid Direct3D staging stride".into());
            }
            let mut bgra = Vec::with_capacity(height as usize * row_bytes);
            for row in 0..height as usize {
                let address = mapped
                    .pData
                    .cast::<u8>()
                    .add(row * mapped.RowPitch as usize);
                bgra.extend_from_slice(std::slice::from_raw_parts(address, row_bytes));
            }
            self.context.Unmap(&target.staging, 0);
            Ok(Arc::new(Pixels {
                width,
                height,
                bgra,
            }))
        }
    }
}

fn texture_desc(size: FrameSize) -> D3D11_TEXTURE2D_DESC {
    D3D11_TEXTURE2D_DESC {
        Width: size.width,
        Height: size.height,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        ..D3D11_TEXTURE2D_DESC::default()
    }
}

fn sampler(device: &ID3D11Device, filter: D3D11_FILTER) -> Result<ID3D11SamplerState> {
    let desc = D3D11_SAMPLER_DESC {
        Filter: filter,
        AddressU: D3D11_TEXTURE_ADDRESS_CLAMP,
        AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
        AddressW: D3D11_TEXTURE_ADDRESS_CLAMP,
        MaxAnisotropy: 1,
        ComparisonFunc: D3D11_COMPARISON_NEVER,
        MaxLOD: f32::MAX,
        ..D3D11_SAMPLER_DESC::default()
    };
    let mut output = None;
    // SAFETY: The descriptor and writable output stay live during creation.
    unsafe { device.CreateSamplerState(&desc, Some(&mut output)) }
        .map_err(|err| format!("create shader sampler: {err}"))?;
    output.ok_or("missing Direct3D sampler".into())
}
