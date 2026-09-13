use std::{collections::BTreeMap, io};
use wasmi::{Engine, Linker, Memory, Module, Store, TypedFunc};

pub(super) struct Host {
    store: Store<()>,
    memory: Memory,
    allocate: TypedFunc<u32, u32>,
    deallocate: TypedFunc<(u32, u32), ()>,
    format: Option<TypedFunc<(u32, u32), u32>>,
    lint: Option<TypedFunc<(u32, u32), u32>>,
    compile: Option<TypedFunc<(u32, u32), u32>>,
}

impl Host {
    pub(super) fn load(
        bytes: &[u8],
        format: bool,
        lint: bool,
        compile: bool,
        configuration: &BTreeMap<String, serde_json::Value>,
    ) -> io::Result<Self> {
        let engine = Engine::default();
        let module = Module::new(&engine, bytes).map_err(io::Error::other)?;
        let mut store = Store::new(&engine, ());

        let instance = Linker::new(&engine)
            .instantiate_and_start(&mut store, &module)
            .map_err(io::Error::other)?;

        let memory = instance
            .get_memory(&store, "memory")
            .ok_or_else(|| io::Error::other("graft exports no memory"))?;

        let allocate = instance
            .get_typed_func(&store, "instar_allocate")
            .map_err(io::Error::other)?;

        let deallocate = instance
            .get_typed_func(&store, "instar_deallocate")
            .map_err(io::Error::other)?;

        let hook = |enabled: bool, name| {
            enabled
                .then(|| instance.get_typed_func(&store, name))
                .transpose()
                .map_err(io::Error::other)
        };

        let format = hook(format, "instar_format")?;
        let lint = hook(lint, "instar_lint")?;
        let compile = hook(compile, "instar_compile")?;

        let mut host = Self {
            store,
            memory,
            allocate,
            deallocate,
            format,
            lint,
            compile,
        };

        if !configuration.is_empty() {
            let configure = instance
                .get_typed_func::<(u32, u32), ()>(&host.store, "instar_configure")
                .map_err(io::Error::other)?;

            let configuration = serde_json::to_vec(configuration).map_err(io::Error::other)?;
            let input = host.push(&configuration)?;

            configure
                .call(&mut host.store, input)
                .map_err(io::Error::other)?;

            host.deallocate
                .call(&mut host.store, input)
                .map_err(io::Error::other)?;
        }

        Ok(host)
    }

    fn push(&mut self, bytes: &[u8]) -> io::Result<(u32, u32)> {
        let length = u32::try_from(bytes.len()).map_err(io::Error::other)?;

        let pointer = self
            .allocate
            .call(&mut self.store, length)
            .map_err(io::Error::other)?;

        self.memory
            .write(&mut self.store, pointer as usize, bytes)
            .map_err(io::Error::other)?;

        Ok((pointer, length))
    }

    pub(super) fn invoke(&mut self, name: &str, source: &[u8]) -> io::Result<Option<Vec<u8>>> {
        let function = match name {
            "instar_format" => self.format,
            "instar_lint" => self.lint,
            "instar_compile" => self.compile,
            _ => return Err(io::Error::other("unknown graft hook")),
        };

        let Some(function) = function else {
            return Ok(None);
        };

        let input = self.push(source)?;

        let header = function
            .call(&mut self.store, input)
            .map_err(io::Error::other)?;

        let mut bytes = [0; 12];

        self.memory
            .read(&self.store, header as usize, &mut bytes)
            .map_err(io::Error::other)?;

        let pointer = u32::from_le_bytes(bytes[0..4].try_into().expect("four bytes"));
        let length = u32::from_le_bytes(bytes[4..8].try_into().expect("four bytes"));
        let success = u32::from_le_bytes(bytes[8..12].try_into().expect("four bytes"));

        if length as usize > super::RESPONSE_LIMIT || success > 1 {
            return Err(io::Error::other("invalid graft response header"));
        }

        let end = (pointer as usize)
            .checked_add(length as usize)
            .ok_or_else(|| io::Error::other("graft response overflows memory"))?;

        let output = self
            .memory
            .data(&self.store)
            .get(pointer as usize..end)
            .ok_or_else(|| io::Error::other("graft response is outside memory"))?
            .to_vec();

        self.deallocate
            .call(&mut self.store, (pointer, length))
            .map_err(io::Error::other)?;

        self.deallocate
            .call(&mut self.store, (header, 12))
            .map_err(io::Error::other)?;

        self.deallocate
            .call(&mut self.store, input)
            .map_err(io::Error::other)?;

        if success == 0 {
            return Err(io::Error::other(
                String::from_utf8(output).map_err(io::Error::other)?,
            ));
        }

        Ok(Some(output))
    }
}
