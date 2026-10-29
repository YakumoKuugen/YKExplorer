// Reads a PDB with the DIA SDK and prints per-compiland function bytes.
// Usage: pdb_sizes.exe <path-to-pdb> <path-to-msdia140.dll>

#include <windows.h>
#include <dia2.h>
#include <diacreate.h>
#include <cstdio>
#include <string>

static std::wstring take_bstr(BSTR value) {
    std::wstring result = value ? value : L"";
    if (value) {
        SysFreeString(value);
    }
    return result;
}

int wmain(int argc, wchar_t **argv) {
    if (argc < 3) {
        std::fwprintf(stderr, L"usage: pdb_sizes.exe <pdb> <msdia140.dll>\n");
        return 2;
    }
    if (FAILED(CoInitialize(nullptr))) {
        return 3;
    }

    IDiaDataSource *source = nullptr;
    HRESULT hr = NoRegCoCreate(argv[2], CLSID_DiaSource, IID_IDiaDataSource,
                               reinterpret_cast<void **>(&source));
    if (FAILED(hr) || !source) {
        std::fwprintf(stderr, L"DiaSource failed: 0x%08X\n", hr);
        return 6;
    }
    hr = source->loadDataFromPdb(argv[1]);
    if (FAILED(hr)) {
        std::fwprintf(stderr, L"loadDataFromPdb failed: 0x%08X\n", hr);
        return 7;
    }
    IDiaSession *session = nullptr;
    hr = source->openSession(&session);
    if (FAILED(hr) || !session) {
        std::fwprintf(stderr, L"openSession failed: 0x%08X\n", hr);
        return 8;
    }
    IDiaSymbol *global = nullptr;
    hr = session->get_globalScope(&global);
    if (FAILED(hr) || !global) {
        std::fwprintf(stderr, L"get_globalScope failed: 0x%08X\n", hr);
        return 9;
    }

    IDiaEnumSymbols *compilands = nullptr;
    hr = global->findChildren(SymTagCompiland, nullptr, nsNone, &compilands);
    if (FAILED(hr) || !compilands) {
        std::fwprintf(stderr, L"findChildren(compiland) failed: 0x%08X\n", hr);
        return 10;
    }

    ULONGLONG grand_total = 0;
    ULONG fetched = 0;
    IDiaSymbol *compiland = nullptr;
    while (compilands->Next(1, &compiland, &fetched) == S_OK && fetched == 1) {
        BSTR name = nullptr;
        compiland->get_name(&name);
        std::wstring module = take_bstr(name);
        ULONGLONG compiland_length = 0;
        compiland->get_length(&compiland_length);

        IDiaEnumSymbols *functions = nullptr;
        ULONGLONG total = 0;
        ULONGLONG count = 0;
        if (SUCCEEDED(compiland->findChildren(SymTagFunction, nullptr, nsNone, &functions)) &&
            functions) {
            IDiaSymbol *function = nullptr;
            while (functions->Next(1, &function, &fetched) == S_OK && fetched == 1) {
                ULONGLONG length = 0;
                if (SUCCEEDED(function->get_length(&length))) {
                    total += length;
                }
                ++count;
                function->Release();
            }
            functions->Release();
        }
        grand_total += total;
        std::wprintf(L"%llu\t%llu\t%llu\t%s\n", total, count, compiland_length, module.c_str());
        compiland->Release();
    }

    // Public symbols carry RVAs even when LTO stripped per-function sizes.
    IDiaEnumSymbols *publics = nullptr;
    if (SUCCEEDED(global->findChildren(SymTagPublicSymbol, nullptr, nsNone, &publics)) &&
        publics) {
        IDiaSymbol *symbol = nullptr;
        while (publics->Next(1, &symbol, &fetched) == S_OK && fetched == 1) {
            DWORD rva = 0;
            ULONGLONG length = 0;
            symbol->get_relativeVirtualAddress(&rva);
            symbol->get_length(&length);
            BSTR name = nullptr;
            symbol->get_undecoratedName(&name);
            if (!name) {
                symbol->get_name(&name);
            }
            std::wstring text = take_bstr(name);
            std::wprintf(L"P\t%u\t%llu\t%s\n", rva, length, text.c_str());
            symbol->Release();
        }
        publics->Release();
    }

    std::fwprintf(stderr, L"total_function_bytes=%llu\n", grand_total);
    compilands->Release();
    global->Release();
    session->Release();
    source->Release();
    CoUninitialize();
    return 0;
}
