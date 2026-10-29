// Dumps PDB section contributions (per object file / compiland) using DIA.
// Usage: pdb_contribs.exe <pdb> <msdia140.dll>

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
        return 4;
    }
    hr = source->loadDataFromPdb(argv[1]);
    if (FAILED(hr)) {
        std::fwprintf(stderr, L"loadDataFromPdb failed: 0x%08X\n", hr);
        return 5;
    }
    IDiaSession *session = nullptr;
    hr = source->openSession(&session);
    if (FAILED(hr) || !session) {
        std::fwprintf(stderr, L"openSession failed: 0x%08X\n", hr);
        return 6;
    }
    IDiaEnumTables *tables = nullptr;
    hr = session->getEnumTables(&tables);
    if (FAILED(hr) || !tables) {
        std::fwprintf(stderr, L"getEnumTables failed: 0x%08X\n", hr);
        return 7;
    }

    ULONG fetched = 0;
    IDiaTable *table = nullptr;
    while (tables->Next(1, &table, &fetched) == S_OK && fetched == 1) {
        BSTR table_name = nullptr;
        table->get_name(&table_name);
        std::wstring name = take_bstr(table_name);
        IDiaEnumSectionContribs *contribs = nullptr;
        hr = table->QueryInterface(IID_IDiaEnumSectionContribs,
                                   reinterpret_cast<void **>(&contribs));
        if (SUCCEEDED(hr) && contribs) {
            std::fwprintf(stderr, L"section table: %s\n", name.c_str());
            IDiaSectionContrib *contrib = nullptr;
            while (contribs->Next(1, &contrib, &fetched) == S_OK && fetched == 1) {
                DWORD section = 0;
                DWORD length = 0;
                DWORD rva = 0;
                BOOL code = FALSE;
                contrib->get_addressSection(&section);
                contrib->get_length(&length);
                contrib->get_relativeVirtualAddress(&rva);
                contrib->get_code(&code);
                IDiaSymbol *compiland = nullptr;
                std::wstring module;
                if (SUCCEEDED(contrib->get_compiland(&compiland)) && compiland) {
                    BSTR module_name = nullptr;
                    compiland->get_name(&module_name);
                    module = take_bstr(module_name);
                    compiland->Release();
                }
                std::wprintf(L"C\t%u\t%u\t%u\t%u\t%s\n", section, rva, length,
                             code ? 1 : 0, module.c_str());
                contrib->Release();
            }
            contribs->Release();
        }
        table->Release();
    }
    tables->Release();
    session->Release();
    source->Release();
    CoUninitialize();
    return 0;
}
